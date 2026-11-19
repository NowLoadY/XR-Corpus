//! Read the host's bookmark archive on demand. The archive owns the words;
//! XR Corpus owns the domain switches and prompt budgets.
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use xr_corpus_core::{
    BASE_LANGUAGE_COLUMNS, CorpusActivation, GraphDomain, GraphNode, GraphSnapshot, GraphStore,
    WORDBOOK_DOMAIN_ID, language_index,
};

pub(super) struct Wordbooks {
    path: PathBuf,
    db: Option<Connection>,
    data_version: Option<i64>,
    revision: Option<i64>,
    missing: bool,
}

impl Wordbooks {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            db: None,
            data_version: None,
            revision: None,
            missing: false,
        }
    }

    pub fn refresh(&mut self, graph: &GraphStore) -> Result<(), String> {
        if !self.path.try_exists().map_err(|e| e.to_string())? {
            if !self.missing {
                graph.replace_wordbooks(empty_wordbooks())?;
                self.db = None;
                self.data_version = None;
                self.revision = None;
                self.missing = true;
            }
            return Ok(());
        }
        self.missing = false;
        if self.db.is_none() {
            self.db = Some(
                Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .map_err(|e| e.to_string())?,
            );
        }
        let db = self.db.as_mut().unwrap();
        let version: i64 = db
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        if self.data_version == Some(version) {
            return Ok(());
        }
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let has_revision: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='vocabulary_revision')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        let revision = if has_revision {
            Some(
                tx.query_row(
                    "SELECT revision FROM vocabulary_revision WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        // Older archives use SQLite's data version. With newer ones, unrelated
        // history writes never require reading the wordbooks again.
        if revision.is_none() || revision != self.revision {
            let snapshot = read(&tx).map_err(|e| e.to_string())?;
            graph.replace_wordbooks(snapshot)?;
        }
        self.data_version = Some(version);
        self.revision = revision;
        Ok(())
    }
}

fn empty_wordbooks() -> GraphSnapshot {
    GraphSnapshot {
        domains: vec![GraphDomain {
            id: WORDBOOK_DOMAIN_ID.into(),
            title: "Vocabulary".into(),
            enabled: true,
            parent_id: None,
            cue_words: None,
        }],
        ..Default::default()
    }
}

fn read(db: &Connection) -> rusqlite::Result<GraphSnapshot> {
    let mut graph = empty_wordbooks();
    let mut folders = db.prepare("SELECT id,name FROM vocabulary_folders ORDER BY id")?;
    for folder in folders.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })? {
        let (id, name) = folder?;
        let title: String = name
            .trim()
            .replace(['\r', '\n'], " ")
            .chars()
            .take(256)
            .collect();
        graph.domains.push(GraphDomain {
            id: format!("{WORDBOOK_DOMAIN_ID}.{id}"),
            parent_id: Some(WORDBOOK_DOMAIN_ID.into()),
            title: if title.is_empty() {
                "Default wordbook".into()
            } else {
                title
            },
            enabled: false,
            cue_words: None,
        });
    }
    let mut words = db.prepare(
        "SELECT folder_id,source_lang,source,id,target_lang,translated FROM vocabulary
        ORDER BY folder_id,source_lang,source,id",
    )?;
    let mut rows = words.query([])?;
    let mut last = None;
    while let Some(row) = rows.next()? {
        let folder: i64 = row.get(0)?;
        let lang: String = row.get(1)?;
        let source: String = row.get(2)?;
        let Some(column) = language_index(&lang) else {
            continue;
        };
        if !valid_value(&source) {
            continue;
        }
        let key = (folder, lang, source);
        if last.as_ref() != Some(&key) {
            let id: i64 = row.get(3)?;
            let mut values = vec![String::new(); BASE_LANGUAGE_COLUMNS.max(column + 1)];
            values[column] = key.2.clone();
            graph.nodes.push(GraphNode {
                id: format!("{WORDBOOK_DOMAIN_ID}.word.{id}"),
                domain_id: format!("{WORDBOOK_DOMAIN_ID}.{folder}"),
                title: key.2.chars().take(256).collect(),
                enabled: true,
                promptable: true,
                activation: CorpusActivation::Always,
                inherit_domain: true,
                priority: 20,
                values,
                recognition_aliases: Default::default(),
            });
            last = Some(key);
        }
        let target: String = row.get(4)?;
        let translated: String = row.get(5)?;
        if let Some(index) = language_index(&target)
            && index != column
            && valid_value(&translated)
        {
            let values = &mut graph.nodes.last_mut().unwrap().values;
            if index >= values.len() {
                values.resize(index + 1, String::new());
            }
            values[index] = translated;
        }
    }
    Ok(graph)
}

fn valid_value(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains(['\r', '\n']) && value.chars().count() <= 512
}
