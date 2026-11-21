// SPDX-License-Identifier: AGPL-3.0-or-later
//! `unemu recipe inputs` (recipes §6.15): the inputs a user supplies for a
//! recipe, as a Logiqx DAT for a ROM manager, a CSV, or a checksum file for
//! `sha256sum -c`. The three forms are the catalogues a library reads
//! (recipes §3.4), and they describe one folder: each file input at its
//! canonical `name`, each file-set entry at its set's name, `/` and its
//! path. A ROM manager that rebuilds the DAT's one game makes that folder,
//! `sha256sum -c` run in it checks it, and the folder is a library a build
//! finds every input in.
//!
//! Inputs with `bundle = true` are left out: their bytes travel with the
//! recipe, so no user gathers them. Members of an `unpack` step are not
//! inputs and are not listed, even those whose size and hash the recipe
//! states. Inputs come in the order of their ids (a TOML table's keys have
//! no order of their own), a file set's entries in the order the set lists
//! them. Nothing is read but the recipe,
//! which must pass the checks of recipes §6.11 first.
//!
//! The forms:
//!
//! - **DAT**: the XML of the Logiqx datafile DTD as the dump catalogues
//!   write it, one `<game>` named after the artifact (the last component of
//!   its repository and its tag), one `<rom>` per row with `name`, `size`,
//!   then `crc`, `md5`, `sha1` when the recipe states them and `sha256`,
//!   which the DTD lacks and the ROM managers that read SHA-256 take.
//! - **CSV**: a header `path,size,sha256,crc32,sha1,md5` and one line per
//!   row, empty fields where the recipe states no weaker hash, a field
//!   quoted when it holds a comma or a quote (RFC 4180), LF line ends.
//! - **sha256sums**: `<hex>  <path>`, two spaces (text mode), as
//!   `sha256sum` writes and `sha256sum -c` reads. A recipe's names and
//!   paths hold no backslash and no control character, so no line needs
//!   the escaped form.

use crate::build::tag;
use crate::model::{InputForm, Recipe};
use crate::{Opened, Providers, Report};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// The form `recipe inputs` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Dat,
    Csv,
    Sha256sums,
}

impl Format {
    /// The word of `--format`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "dat" => Some(Self::Dat),
            "csv" => Some(Self::Csv),
            "sha256sums" => Some(Self::Sha256sums),
            _ => None,
        }
    }
}

/// One file a user supplies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Where the file lies in the inputs' folder.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub crc32: Option<String>,
    pub md5: Option<String>,
    pub sha1: Option<String>,
}

/// The inputs of one recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inputs {
    /// The repository the recipe builds.
    pub name: String,
    pub tag: String,
    pub rows: Vec<Row>,
}

/// What [`inputs_path`] gives for one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    Inputs(Inputs),
    /// The file does not pass the checks: their report, with findings.
    Refused(Report),
    /// The file passes the checks but is a work file, which has no inputs.
    NotRecipe,
    /// Two inputs that one folder cannot hold: why.
    Clash(String),
}

/// Check one recipe file, as `recipe check` does, and list the inputs of a
/// recipe that passes.
pub fn inputs_path(path: &Path, providers: Providers<'_>) -> io::Result<Listing> {
    Ok(match crate::load(path, providers)? {
        Opened::Refused(report) => Listing::Refused(report),
        Opened::Work => Listing::NotRecipe,
        Opened::Recipe(recipe, _, _) => match inputs(&recipe) {
            Ok(inputs) => Listing::Inputs(inputs),
            Err(clash) => Listing::Clash(clash),
        },
    })
}

/// The rows of a recipe that passed the checks: inputs by id, a file set's
/// entries in its order. Two inputs at one path with the same size and
/// hash are one row; with other bytes, or a file at a path another input
/// needs as a folder, the folder the rows describe cannot hold both, and
/// the error names the path and both inputs.
pub fn inputs(recipe: &Recipe) -> Result<Inputs, String> {
    let mut rows: Vec<(&str, Row)> = Vec::new();
    for input in &recipe.inputs {
        if input.bundle.as_ref().is_some_and(|b| b.value) {
            continue;
        }
        let Some(name) = &input.name else { continue };
        match &input.form {
            InputForm::File {
                size: Some(size),
                sha256: Some(sha256),
            } => rows.push((
                &input.id.value,
                Row {
                    path: name.value.clone(),
                    size: size.value,
                    sha256: sha256.value.clone(),
                    crc32: input.crc32.as_ref().map(|v| v.value.clone()),
                    md5: input.md5.as_ref().map(|v| v.value.clone()),
                    sha1: input.sha1.as_ref().map(|v| v.value.clone()),
                },
            )),
            InputForm::FileSet { files } => {
                for entry in files {
                    let (Some(size), Some(sha256)) = (&entry.size, &entry.sha256) else {
                        continue;
                    };
                    rows.push((
                        &input.id.value,
                        Row {
                            path: format!("{}/{}", name.value, entry.path.value),
                            size: size.value,
                            sha256: sha256.value.clone(),
                            crc32: None,
                            md5: None,
                            sha1: None,
                        },
                    ));
                }
            }
            _ => {}
        }
    }
    let mut at: BTreeMap<&str, (&str, &Row)> = BTreeMap::new();
    let mut kept = Vec::new();
    for (index, (id, row)) in rows.iter().enumerate() {
        match at.get(row.path.as_str()) {
            Some((_, held)) if held.size == row.size && held.sha256 == row.sha256 => {}
            Some((other, _)) => {
                return Err(format!(
                    "the inputs `{other}` and `{id}` both lie at {} with other bytes; one folder cannot hold both",
                    row.path
                ))
            }
            None => {
                at.insert(&row.path, (id, row));
                kept.push(index);
            }
        }
    }
    // A file where another input needs a folder.
    for (id, row) in &rows {
        let mut folder = row.path.as_str();
        while let Some((parent, _)) = folder.rsplit_once('/') {
            if let Some((other, _)) = at.get(parent) {
                return Err(format!(
                    "the input `{other}` is a file at {parent}, where `{id}` needs a folder for {}; one folder cannot hold both",
                    row.path
                ));
            }
            folder = parent;
        }
    }
    let rows = kept.into_iter().map(|i| rows[i].1.clone()).collect();
    Ok(Inputs {
        name: recipe
            .identity
            .name
            .as_ref()
            .map(|n| n.value.clone())
            .unwrap_or_default(),
        tag: tag(recipe),
        rows,
    })
}

impl Inputs {
    /// The text of `format`.
    pub fn render(&self, format: Format) -> String {
        match format {
            Format::Dat => self.dat(),
            Format::Csv => self.csv(),
            Format::Sha256sums => self
                .rows
                .iter()
                .map(|row| format!("{}  {}\n", row.sha256, row.path))
                .collect(),
        }
    }

    /// The artifact's name and tag, `unemu/media/ibm-pc/kenslab:2.01-r1`.
    fn reference(&self) -> String {
        format!("{}:{}", self.name, self.tag)
    }

    fn dat(&self) -> String {
        let reference = xml(&self.reference());
        let last = self.name.rsplit('/').next().unwrap_or_default();
        let mut out = String::from(concat!(
            "<?xml version=\"1.0\"?>\n",
            "<!DOCTYPE datafile PUBLIC \"-//Logiqx//DTD ROM Management Datafile//EN\" \"http://www.logiqx.com/Dats/datafile.dtd\">\n",
            "<datafile>\n",
            "\t<header>\n",
        ));
        out.push_str(&format!("\t\t<name>{}</name>\n", xml(&self.name)));
        out.push_str(&format!("\t\t<description>{reference}</description>\n"));
        out.push_str(&format!("\t\t<version>{}</version>\n", xml(&self.tag)));
        out.push_str("\t\t<author>unemu recipe inputs</author>\n");
        out.push_str("\t</header>\n");
        out.push_str(&format!(
            "\t<game name=\"{}\">\n",
            xml(&format!("{last}-{}", self.tag))
        ));
        out.push_str(&format!("\t\t<description>{reference}</description>\n"));
        for row in &self.rows {
            out.push_str(&format!(
                "\t\t<rom name=\"{}\" size=\"{}\"",
                xml(&row.path),
                row.size
            ));
            for (key, value) in [("crc", &row.crc32), ("md5", &row.md5), ("sha1", &row.sha1)] {
                if let Some(value) = value {
                    out.push_str(&format!(" {key}=\"{value}\""));
                }
            }
            out.push_str(&format!(" sha256=\"{}\"/>\n", row.sha256));
        }
        out.push_str("\t</game>\n</datafile>\n");
        out
    }

    fn csv(&self) -> String {
        let mut out = String::from("path,size,sha256,crc32,sha1,md5\n");
        for row in &self.rows {
            let weak = |v: &Option<String>| v.clone().unwrap_or_default();
            out.push_str(&format!(
                "{},{},{},{},{},{}\n",
                csv(&row.path),
                row.size,
                row.sha256,
                weak(&row.crc32),
                weak(&row.sha1),
                weak(&row.md5)
            ));
        }
        out
    }
}

/// Text or an attribute value in XML: the five characters XML reserves as
/// references. A recipe's names hold no control character.
fn xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// A CSV field, quoted when it holds a comma or a quote.
fn csv(field: &str) -> String {
    if field.contains([',', '"']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    const SHA_A: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const SHA_B: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const SHA_C: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    fn recipe() -> Recipe {
        let text = format!(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/demo\"\nplatform = \"ibm-pc\"\nrelease = \"1.0\"\nrevision = 2\n\n\
             [input.zip]\nname = \"A&B, \\\"1\\\".ZIP\"\nsize = 10\nsha256 = \"{SHA_A}\"\ncrc32 = \"0badf00d\"\nsha1 = \"{}\"\n\n\
             [input.text]\nname = \"OWN.TXT\"\nsize = 3\nsha256 = \"{SHA_C}\"\nbundle = true\n\n\
             [input.game]\nname = \"GAME\"\nfiles = [\n    {{ path = \"A.EXE\", size = 5, sha256 = \"{SHA_B}\" }},\n    {{ path = \"SUB/<B>.DAT\", size = 6, sha256 = \"{SHA_C}\" }},\n]\n",
            "a".repeat(40)
        );
        match crate::parse_text(&text) {
            Some(Document::Recipe(recipe)) => *recipe,
            _ => panic!("the test recipe does not parse"),
        }
    }

    #[test]
    fn the_three_forms_list_the_same_rows_without_bundled_inputs() {
        let inputs = inputs(&recipe()).unwrap();
        assert_eq!(
            inputs.render(Format::Sha256sums),
            format!("{SHA_B}  GAME/A.EXE\n{SHA_C}  GAME/SUB/<B>.DAT\n{SHA_A}  A&B, \"1\".ZIP\n")
        );
        assert_eq!(
            inputs.render(Format::Csv),
            format!(
                "path,size,sha256,crc32,sha1,md5\nGAME/A.EXE,5,{SHA_B},,,\nGAME/SUB/<B>.DAT,6,{SHA_C},,,\n\"A&B, \"\"1\"\".ZIP\",10,{SHA_A},0badf00d,{},\n",
                "a".repeat(40)
            )
        );
        assert_eq!(
            inputs.render(Format::Dat),
            format!(
                "<?xml version=\"1.0\"?>\n\
                 <!DOCTYPE datafile PUBLIC \"-//Logiqx//DTD ROM Management Datafile//EN\" \"http://www.logiqx.com/Dats/datafile.dtd\">\n\
                 <datafile>\n\
                 \t<header>\n\
                 \t\t<name>private/media/ibm-pc/demo</name>\n\
                 \t\t<description>private/media/ibm-pc/demo:1.0-r2</description>\n\
                 \t\t<version>1.0-r2</version>\n\
                 \t\t<author>unemu recipe inputs</author>\n\
                 \t</header>\n\
                 \t<game name=\"demo-1.0-r2\">\n\
                 \t\t<description>private/media/ibm-pc/demo:1.0-r2</description>\n\
                 \t\t<rom name=\"GAME/A.EXE\" size=\"5\" sha256=\"{SHA_B}\"/>\n\
                 \t\t<rom name=\"GAME/SUB/&lt;B&gt;.DAT\" size=\"6\" sha256=\"{SHA_C}\"/>\n\
                 \t\t<rom name=\"A&amp;B, &quot;1&quot;.ZIP\" size=\"10\" crc=\"0badf00d\" sha1=\"{}\" sha256=\"{SHA_A}\"/>\n\
                 \t</game>\n\
                 </datafile>\n",
                "a".repeat(40)
            )
        );
    }

    fn parsed(text: &str) -> Recipe {
        let text = format!(
            "recipe = \"1.0\"\n\n[media]\nname = \"private/media/ibm-pc/demo\"\nplatform = \"ibm-pc\"\nrevision = 1\n\n{text}"
        );
        match crate::parse_text(&text) {
            Some(Document::Recipe(recipe)) => *recipe,
            _ => panic!("the test recipe does not parse"),
        }
    }

    #[test]
    fn two_inputs_one_folder_cannot_hold_are_refused() {
        let file = |id: &str, name: &str, sha: &str| {
            format!("[input.{id}]\nname = \"{name}\"\nsize = 1\nsha256 = \"{sha}\"\n\n")
        };
        // The same name and other bytes.
        let clash = inputs(&parsed(&format!(
            "{}{}",
            file("a", "X.BIN", SHA_A),
            file("b", "X.BIN", SHA_B)
        )))
        .unwrap_err();
        assert_eq!(
            clash,
            "the inputs `a` and `b` both lie at X.BIN with other bytes; one folder cannot hold both"
        );
        // The same name and the same bytes: one row.
        let same = inputs(&parsed(&format!(
            "{}{}",
            file("a", "X.BIN", SHA_A),
            file("b", "X.BIN", SHA_A)
        )))
        .unwrap();
        assert_eq!(same.rows.len(), 1);
        // A file where a file set needs its folder.
        let set = format!(
            "[input.set]\nname = \"GAME\"\nfiles = [\n    {{ path = \"SUB/A.EXE\", size = 5, sha256 = \"{SHA_B}\" }},\n]\n\n"
        );
        let clash = inputs(&parsed(&format!("{}{set}", file("game", "GAME", SHA_A)))).unwrap_err();
        assert_eq!(
            clash,
            "the input `game` is a file at GAME, where `set` needs a folder for GAME/SUB/A.EXE; one folder cannot hold both"
        );
    }

    #[test]
    fn formats_are_named_by_their_words() {
        assert_eq!(Format::parse("dat"), Some(Format::Dat));
        assert_eq!(Format::parse("csv"), Some(Format::Csv));
        assert_eq!(Format::parse("sha256sums"), Some(Format::Sha256sums));
        assert_eq!(Format::parse("SHA256SUMS"), None);
        assert_eq!(Format::parse("json"), None);
    }
}
