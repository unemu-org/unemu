// SPDX-License-Identifier: AGPL-3.0-or-later
//! The native macOS application menu and the system quit hook.
//!
//! Builds the menu that [`crate::app_menu`] describes with AppKit's NSMenu and
//! NSMenuItem, and hands AppKit's quit question (NSApplicationDelegate
//! `applicationShouldTerminate:`) to the shell's Exit path; see
//! `backlog/docs/design/machine-images.md` §7. All unsafe code of the shell's
//! AppKit integration stays in this module.
//!
//! winit owns the application delegate, and its event loop checks that the
//! delegate is still its own object, so the delegate cannot be replaced. winit's
//! delegate class does not answer `applicationShouldTerminate:`, so this module
//! adds that one method to the class at run time (Objective-C runtime,
//! `class_addMethod`). If a later winit answers it itself, its method is left
//! alone, a system quit keeps winit's behaviour, and startup says so on stderr.

use crate::{
    app_menu::{self, MenuAction, MenuEntry, NativeRequest},
    shell_model::Command,
};
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Imp, NSObject, Sel},
    sel, DefinedClass, Encode, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSApplicationTerminateReply, NSEventModifierFlags, NSMenu, NSMenuItem,
};
use objc2_foundation::{NSArray, NSObjectNSDelayedPerforming, NSRunLoopCommonModes, NSString};
use std::{
    cell::{Cell, RefCell},
    ffi::CString,
};

/// Hands a request to the shell; false when the shell cannot take it now.
type Sink = Box<dyn Fn(NativeRequest) -> bool>;

struct Ivars {
    sink: Sink,
    /// The shell command of each menu item, indexed by the item's tag.
    commands: Vec<Command>,
    /// The answer the next `replyToTerminate:` gives AppKit.
    reply: Cell<Option<bool>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `MenuTarget` does
    // not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "UnemupcApplicationMenuTarget"]
    #[ivars = Ivars]
    struct MenuTarget;

    impl MenuTarget {
        // SAFETY: AppKit sends a menu item's action with the item as its only
        // argument and no return value.
        #[unsafe(method(runShellCommand:))]
        fn run_shell_command(&self, item: Option<&NSMenuItem>) {
            let ivars = self.ivars();
            let command = item
                .and_then(|item| usize::try_from(item.tag()).ok())
                .and_then(|tag| ivars.commands.get(tag));
            if let Some(&command) = command {
                (ivars.sink)(NativeRequest::Command(command));
            }
        }

        // SAFETY: only `reply_to_terminate` schedules this selector, with one
        // nil object argument and no return value.
        #[unsafe(method(replyToTerminate:))]
        fn reply_to_terminate(&self, _argument: Option<&AnyObject>) {
            if let Some(terminate) = self.ivars().reply.take() {
                NSApplication::sharedApplication(self.mtm())
                    .replyToApplicationShouldTerminate(terminate);
            }
        }
    }
);

thread_local! {
    /// Menu items hold their target weakly, so the target lives here.
    static TARGET: RefCell<Option<Retained<MenuTarget>>> = const { RefCell::new(None) };
}

/// Install the application menu and the system quit hook on the main thread,
/// after winit has created its event loop. `sink` hands each request to the
/// shell and returns false when the shell cannot take it.
pub(crate) fn install(sink: impl Fn(NativeRequest) -> bool + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let entries = app_menu::application_menu();
    let commands = entries
        .iter()
        .filter_map(|entry| match entry {
            MenuEntry::Item {
                action: MenuAction::Shell(command),
                ..
            } => Some(*command),
            _ => None,
        })
        .collect();
    let target = MenuTarget::alloc(mtm).set_ivars(Ivars {
        sink: Box::new(sink),
        commands,
        reply: Cell::new(None),
    });
    // SAFETY: NSObject's `init` initializes this direct NSObject subclass,
    // whose ivars are already set.
    let target: Retained<MenuTarget> = unsafe { msg_send![super(target), init] };
    app.setMainMenu(Some(&menu_bar(mtm, &entries, &target)));
    TARGET.set(Some(target));
    hook_system_quit(&app);
}

fn menu_bar(mtm: MainThreadMarker, entries: &[MenuEntry], target: &MenuTarget) -> Retained<NSMenu> {
    let target: &AnyObject = target;
    let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(app_menu::APP_NAME));
    let mut tag = 0;
    for entry in entries {
        let MenuEntry::Item { title, key, action } = *entry else {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            continue;
        };
        let selector = match action {
            MenuAction::Shell(_) => sel!(runShellCommand:),
            MenuAction::Application(name) => Sel::register(name),
        };
        let key_equivalent = NSString::from_str(key.map_or("", |key| key.key));
        // SAFETY: `selector` names an action method that takes the sender:
        // the target's `runShellCommand:`, or an NSApplication action that the
        // responder chain delivers to the application.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                Some(selector),
                &key_equivalent,
            )
        };
        if let Some(key) = key {
            let mut modifiers = NSEventModifierFlags::Command;
            if key.option {
                modifiers |= NSEventModifierFlags::Option;
            }
            item.setKeyEquivalentModifierMask(modifiers);
        }
        if let MenuAction::Shell(_) = action {
            item.setTag(tag);
            tag += 1;
            // SAFETY: the target is a `MenuTarget`, which implements
            // `runShellCommand:`, and `TARGET` keeps it alive for the process.
            unsafe { item.setTarget(Some(target)) };
        }
        menu.addItem(&item);
    }
    let application = NSMenuItem::new(mtm);
    application.setSubmenu(Some(&menu));
    let bar = NSMenu::new(mtm);
    bar.addItem(&application);
    bar
}

/// `applicationShouldTerminate:` for winit's delegate. The shell's Exit
/// decides, and `reply_to_terminate` answers later.
///
/// The quit is cancelled when the shell cannot answer: while an application-modal
/// panel such as a file dialog runs inside a shell pass, no further pass can
/// run until it closes, and when the shell cannot take the request at all.
/// Terminating at once instead would end the process inside winit's event
/// handling and skip persistence.
extern "C-unwind" fn application_should_terminate(
    _delegate: &AnyObject,
    _selector: Sel,
    _sender: Option<&AnyObject>,
) -> NSApplicationTerminateReply {
    let accepted = TARGET.with_borrow(|target| {
        target.as_ref().is_some_and(|target| {
            NSApplication::sharedApplication(target.mtm())
                .modalWindow()
                .is_none()
                && (target.ivars().sink)(NativeRequest::SystemQuit)
        })
    });
    if accepted {
        NSApplicationTerminateReply::TerminateLater
    } else {
        NSApplicationTerminateReply::TerminateCancel
    }
}

type ShouldTerminate =
    extern "C-unwind" fn(&AnyObject, Sel, Option<&AnyObject>) -> NSApplicationTerminateReply;

fn hook_system_quit(app: &NSApplication) {
    let Some(delegate) = app.delegate() else {
        return;
    };
    let object: &AnyObject = (*delegate).as_ref();
    let class = object.class();
    let selector = sel!(applicationShouldTerminate:);
    let answered = class.instance_method(selector).is_some();
    if answered {
        eprintln!(
            "unemupc98: the application delegate already answers applicationShouldTerminate:; \
             Dock Quit and logout will not ask before stopping a running machine"
        );
    }
    debug_assert!(
        !answered,
        "winit's application delegate now answers applicationShouldTerminate:"
    );
    if answered {
        return;
    }
    let Ok(types) = CString::new(format!(
        "{}{}{}{}",
        NSApplicationTerminateReply::ENCODING,
        <&AnyObject>::ENCODING,
        Sel::ENCODING,
        <Option<&AnyObject>>::ENCODING,
    )) else {
        return;
    };
    let function: ShouldTerminate = application_should_terminate;
    // SAFETY: the runtime calls an IMP with the types its encoding declares,
    // and `types` declares exactly `ShouldTerminate`.
    let imp = unsafe { std::mem::transmute::<ShouldTerminate, Imp>(function) };
    // SAFETY: `class` is the registered class of the live delegate, `selector`
    // is registered, and `types` is a NUL-terminated encoding of `imp`. The
    // class answered nothing for `selector` before, so no behaviour it already
    // had changes.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            (class as *const AnyClass).cast_mut(),
            selector,
            imp,
            types.as_ptr(),
        )
    };
    if added.as_bool() {
        // AppKit may note which optional methods a delegate answers when the
        // delegate is set; set it again so the new method is seen.
        app.setDelegate(None);
        app.setDelegate(Some(&delegate));
    }
}

/// Answer the quit question AppKit was told to wait for. AppKit may end the
/// process inside the reply, and winit must not be inside an event then, so
/// the reply runs from the run loop after the current event. Common modes keep
/// it running while AppKit waits in its modal-panel run-loop mode.
pub(crate) fn reply_to_terminate(terminate: bool) {
    TARGET.with_borrow(|target| {
        let Some(target) = target else {
            return;
        };
        target.ivars().reply.set(Some(terminate));
        // SAFETY: Foundation defines this constant for the process lifetime.
        let common = unsafe { NSRunLoopCommonModes };
        let modes = NSArray::from_slice(&[common]);
        // SAFETY: `replyToTerminate:` is a method of the target that takes one
        // optional object; nil is passed.
        unsafe {
            target.performSelector_withObject_afterDelay_inModes(
                sel!(replyToTerminate:),
                None,
                0.0,
                &modes,
            )
        };
    });
}

/// Bring a hidden or background application forward so its quit question can
/// be answered.
pub(crate) fn unhide() {
    TARGET.with_borrow(|target| {
        if let Some(target) = target {
            let app = NSApplication::sharedApplication(target.mtm());
            app.unhide(None);
            // Its replacement, `activate`, needs macOS 14.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    });
}
