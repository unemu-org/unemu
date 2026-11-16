// SPDX-License-Identifier: AGPL-3.0-or-later
//! Host file dialogs that never run their own event loop inside the shell's
//! frame; see `backlog/docs/design/spec.md` §9.4.
//!
//! A synchronous file dialog spins a nested modal event loop until the user
//! answers. On macOS that nested loop dispatches winit's queued events while
//! winit is still inside the handler that drew the frame which opened the
//! dialog, and winit aborts the process. A running machine wakes the event
//! loop all the time, which is likely why the abort came nearly every time
//! with a machine running and only now and then without one.
//!
//! [`FileDialogs`] starts rfd's asynchronous dialog instead and returns at
//! once. On macOS that dialog is a sheet on the window whose completion
//! handler runs from the ordinary main run loop; on Linux the XDG desktop
//! portal answers on a thread of its own. The shell polls the answer once per
//! frame without blocking, and the waker asks egui for a repaint, so the
//! answer reaches the code that asked on a later frame even when nothing else
//! draws. One dialog is open at a time; a second request is refused.

use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};
use winit::window::Window;

/// What the dialog chooses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// One existing file.
    File,
    /// Several existing files, in the order the dialog returns them.
    Files,
    /// One existing folder.
    Folder,
    /// A file name to save under; the file need not exist.
    Save,
}

/// One host file dialog: what it chooses, its title, and where it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub kind: Kind,
    pub title: String,
    /// Named extension filters, in the order the dialog lists them.
    pub filters: Vec<(String, Vec<String>)>,
    pub directory: Option<PathBuf>,
    pub file_name: Option<String>,
}

impl Request {
    pub fn new(kind: Kind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            filters: Vec::new(),
            directory: None,
            file_name: None,
        }
    }

    pub fn filter(mut self, name: &str, extensions: &[&str]) -> Self {
        self.filters.push((
            name.into(),
            extensions
                .iter()
                .map(|&extension| extension.into())
                .collect(),
        ));
        self
    }

    pub fn directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.directory = Some(directory.into());
        self
    }

    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }
}

/// The chosen paths, or `None` when the user cancelled.
pub(crate) type Answer = Option<Vec<PathBuf>>;

/// A dialog's answer, still to come.
pub(crate) type Pending = Pin<Box<dyn Future<Output = Answer>>>;

/// Opens the platform's dialog for a request, attached to the window when
/// there is one, and returns without waiting for the user.
pub(crate) type Host = Box<dyn FnMut(&Request, Option<&Window>) -> Pending>;

/// A second dialog was asked for while one is open.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Busy;

enum State<T> {
    Idle,
    Open(T, Pending),
    /// The dialog answered while it was being opened; the answer still waits
    /// for the next poll, as every other answer does.
    Answered(T, Answer),
}

/// The shell's one host file dialog and who asked for it.
pub(crate) struct FileDialogs<T> {
    host: Host,
    state: State<T>,
    /// Asks egui for a frame when the dialog answers. Made at the first poll;
    /// eframe keeps one context for the application's whole run.
    repaint: Option<Waker>,
    /// A request was refused while the open dialog waits.
    refused: bool,
}

impl<T> FileDialogs<T> {
    /// The operating system's dialogs through rfd.
    pub fn native() -> Self {
        Self::with_host(Box::new(native))
    }

    pub fn with_host(host: Host) -> Self {
        Self {
            host,
            state: State::Idle,
            repaint: None,
            refused: false,
        }
    }

    /// Whether a dialog waits for the user.
    pub fn is_open(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    /// Whether a request was refused while the open dialog waits; cleared
    /// when its answer is delivered.
    pub fn refused(&self) -> bool {
        self.refused
    }

    fn waker(&self) -> &Waker {
        self.repaint.as_ref().unwrap_or(Waker::noop())
    }

    /// Show the dialog for `requester` and return without waiting for it.
    /// The shell polls before it draws each frame, so the dialog's answer
    /// already has egui's repaint waker.
    pub fn open(
        &mut self,
        requester: T,
        request: &Request,
        parent: Option<&Window>,
    ) -> Result<(), Busy> {
        if self.is_open() {
            self.refused = true;
            return Err(Busy);
        }
        let mut answer = (self.host)(request, parent);
        // The first poll starts a dialog that waits for it (the portal's
        // thread) and registers the waker with the platform's answer.
        self.state = match answer.as_mut().poll(&mut Context::from_waker(self.waker())) {
            Poll::Ready(answer) => {
                // Nothing else will wake the shell for an answer that is
                // already here.
                self.waker().wake_by_ref();
                State::Answered(requester, answer)
            }
            Poll::Pending => State::Open(requester, answer),
        };
        Ok(())
    }

    /// The closed dialog's answer with its requester, once; `None` while the
    /// dialog is open or when none was asked for.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<(T, Answer)> {
        if self.repaint.is_none() {
            self.repaint = Some(Waker::from(Arc::new(Repaint(ctx.clone()))));
        }
        let answered = match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => None,
            State::Answered(requester, answer) => Some((requester, answer)),
            State::Open(requester, mut answer) => {
                match answer.as_mut().poll(&mut Context::from_waker(self.waker())) {
                    Poll::Ready(answer) => Some((requester, answer)),
                    Poll::Pending => {
                        self.state = State::Open(requester, answer);
                        None
                    }
                }
            }
        };
        if answered.is_some() {
            self.refused = false;
        }
        answered
    }
}

/// Wakes the shell for one more frame when the dialog answers, from the
/// main run loop on macOS or the portal's thread on Linux.
struct Repaint(egui::Context);

impl Wake for Repaint {
    fn wake(self: Arc<Self>) {
        self.0.request_repaint();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.request_repaint();
    }
}

fn native(request: &Request, parent: Option<&Window>) -> Pending {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(request.title.as_str());
    for (name, extensions) in &request.filters {
        dialog = dialog.add_filter(name.as_str(), extensions.as_slice());
    }
    if let Some(directory) = &request.directory {
        dialog = dialog.set_directory(directory);
    }
    if let Some(name) = &request.file_name {
        dialog = dialog.set_file_name(name.as_str());
    }
    if let Some(window) = parent {
        dialog = dialog.set_parent(window);
    }
    let path = |handle: rfd::FileHandle| handle.path().to_path_buf();
    // The macOS sheet opens at these calls; the portal's thread starts at the
    // first poll.
    match request.kind {
        Kind::File => {
            let answer = dialog.pick_file();
            Box::pin(async move { answer.await.map(|handle| vec![path(handle)]) })
        }
        Kind::Files => {
            let answer = dialog.pick_files();
            Box::pin(async move {
                answer
                    .await
                    .map(|handles| handles.into_iter().map(path).collect())
            })
        }
        Kind::Folder => {
            let answer = dialog.pick_folder();
            Box::pin(async move { answer.await.map(|handle| vec![path(handle)]) })
        }
        Kind::Save => {
            let answer = dialog.save_file();
            Box::pin(async move { answer.await.map(|handle| vec![path(handle)]) })
        }
    }
}

/// A scripted host for tests: it records every request and answers only when
/// the test says so.
#[cfg(test)]
pub(crate) mod fake {
    use super::{Answer, Request};
    use std::{cell::RefCell, rc::Rc, task::Poll};

    #[derive(Clone, Default)]
    pub(crate) struct Host {
        pub requests: Rc<RefCell<Vec<Request>>>,
        answer: Rc<RefCell<Option<Answer>>>,
        waker: Rc<RefCell<Option<std::task::Waker>>>,
    }

    impl Host {
        pub fn boxed(&self) -> super::Host {
            let host = self.clone();
            Box::new(move |request, _| {
                host.requests.borrow_mut().push(request.clone());
                let (answer, waker) = (host.answer.clone(), host.waker.clone());
                Box::pin(std::future::poll_fn(move |cx| {
                    *waker.borrow_mut() = Some(cx.waker().clone());
                    answer
                        .borrow_mut()
                        .take()
                        .map_or(Poll::Pending, Poll::Ready)
                }))
            })
        }

        /// The user closes the dialog with this answer; the platform wakes
        /// the last poll's waker.
        pub fn answer(&self, answer: Answer) {
            *self.answer.borrow_mut() = Some(answer);
            if let Some(waker) = self.waker.borrow_mut().take() {
                waker.wake();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn dialogs() -> (FileDialogs<&'static str>, fake::Host) {
        let host = fake::Host::default();
        (FileDialogs::with_host(host.boxed()), host)
    }

    fn request() -> Request {
        Request::new(Kind::File, "Choose a floppy image").filter("Disk images", &["img"])
    }

    #[test]
    fn an_answer_reaches_its_requester_once_on_a_later_poll() {
        let ctx = egui::Context::default();
        let (mut dialogs, host) = dialogs();
        assert!(!dialogs.is_open());
        assert_eq!(dialogs.poll(&ctx), None);
        dialogs.open("drive A", &request(), None).unwrap();
        assert!(dialogs.is_open());
        assert_eq!(*host.requests.borrow(), [request()]);
        assert_eq!(dialogs.poll(&ctx), None, "the user has not answered yet");
        assert!(dialogs.is_open());
        host.answer(Some(vec!["disk.img".into()]));
        assert_eq!(
            dialogs.poll(&ctx),
            Some(("drive A", Some(vec![PathBuf::from("disk.img")])))
        );
        assert!(!dialogs.is_open());
        assert_eq!(dialogs.poll(&ctx), None, "an answer is delivered once");
    }

    #[test]
    fn cancel_delivers_none_to_the_requester() {
        let ctx = egui::Context::default();
        let (mut dialogs, host) = dialogs();
        dialogs.poll(&ctx);
        dialogs.open("journal", &request(), None).unwrap();
        host.answer(None);
        assert_eq!(dialogs.poll(&ctx), Some(("journal", None)));
        assert!(!dialogs.is_open());
    }

    #[test]
    fn a_second_request_while_one_is_open_is_refused() {
        let ctx = egui::Context::default();
        let (mut dialogs, host) = dialogs();
        dialogs.open("first", &request(), None).unwrap();
        assert!(!dialogs.refused());
        let other = Request::new(Kind::Folder, "Import ROM folder");
        assert_eq!(dialogs.open("second", &other, None), Err(Busy));
        assert!(dialogs.refused());
        assert_eq!(host.requests.borrow().len(), 1, "no second dialog appears");
        host.answer(Some(vec!["a.img".into()]));
        assert_eq!(
            dialogs.poll(&ctx),
            Some(("first", Some(vec![PathBuf::from("a.img")])))
        );
        assert!(
            !dialogs.refused(),
            "the refusal belonged to the closed dialog"
        );
        // Once the first dialog is closed, the next one opens.
        dialogs.open("second", &other, None).unwrap();
        assert_eq!(host.requests.borrow().last(), Some(&other));
    }

    /// A context that counts the repaints asked of it.
    fn counting_context() -> (egui::Context, Arc<AtomicUsize>) {
        let ctx = egui::Context::default();
        let repaints = Arc::new(AtomicUsize::new(0));
        let counter = repaints.clone();
        ctx.set_request_repaint_callback(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        (ctx, repaints)
    }

    #[test]
    fn an_answer_ready_at_once_wakes_the_shell_and_waits_for_the_next_poll() {
        let (ctx, repaints) = counting_context();
        let (mut dialogs, host) = dialogs();
        dialogs.poll(&ctx);
        host.answer(Some(vec!["ready.img".into()]));
        dialogs.open("drive B", &request(), None).unwrap();
        assert!(dialogs.is_open());
        assert!(
            repaints.load(Ordering::SeqCst) > 0,
            "no unrelated frame is needed to take the answer"
        );
        assert_eq!(
            dialogs.poll(&ctx),
            Some(("drive B", Some(vec![PathBuf::from("ready.img")])))
        );
    }

    #[test]
    fn the_answer_asks_egui_for_another_frame() {
        let (ctx, repaints) = counting_context();
        let (mut dialogs, host) = dialogs();
        dialogs.poll(&ctx);
        dialogs.open("drive A", &request(), None).unwrap();
        assert_eq!(repaints.load(Ordering::SeqCst), 0);
        host.answer(None);
        assert!(
            repaints.load(Ordering::SeqCst) > 0,
            "an idle shell draws again to take the answer"
        );
    }
}
