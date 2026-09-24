use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::{CORPUS_SCHEMA, CorpusActivation, CorpusDefinition, CorpusTerm, VRCX_DOMAIN_ID};

const DATABASE_VERSION: i64 = 3;
static NEXT_SEED_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphDomain {
    pub id: String,
    pub title: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphNode {
    pub id: String,
    pub domain_id: String,
    pub subdomain: String,
    pub title: String,
    pub enabled: bool,
    pub promptable: bool,
    pub activation: CorpusActivation,
    pub priority: i32,
    pub values: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphNodeStatePatch {
    pub ids: Vec<String>,
    pub enabled: Option<bool>,
    pub domain_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphEdgeKind {
    Trigger,
    Context,
}

impl GraphEdgeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Context => "context",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "trigger" => Ok(Self::Trigger),
            "context" => Ok(Self::Context),
            _ => Err(format!("invalid graph edge kind {value:?}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source_id: String,
    pub target_id: String,
    pub kind: GraphEdgeKind,
    pub enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub domains: Vec<GraphDomain>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

pub(crate) struct GraphProjection {
    pub corpora: Vec<CorpusDefinition>,
    pub disabled_domains: HashSet<String>,
    pub revision: u64,
}

struct StoreState {
    database: Connection,
    snapshot: GraphSnapshot,
    projection: Arc<GraphProjection>,
}

/// The installed, user-owned graph. Reads during inference use an immutable
/// projection; edits rebuild it in the same transaction as the SQLite write.
#[derive(Clone)]
pub struct GraphStore {
    state: Arc<Mutex<StoreState>>,
}

impl GraphStore {
    pub fn open(runtime_db: &Path, seed_db: &Path) -> Result<Self, String> {
        if !runtime_db.is_file() {
            validate_seed_database(seed_db)?;
            let parent = runtime_db
                .parent()
                .ok_or_else(|| format!("database path has no parent: {}", runtime_db.display()))?;
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| format!("cannot create database seed name: {error}"))?
                .as_nanos();
            let temporary = parent.join(format!(
                ".xr-corpus-{}-{stamp}-{}.sqlite.tmp",
                std::process::id(),
                NEXT_SEED_FILE.fetch_add(1, Ordering::Relaxed)
            ));
            if let Err(error) = fs::copy(seed_db, &temporary) {
                let _ = fs::remove_file(&temporary);
                return Err(format!(
                    "cannot seed database {}: {error}",
                    temporary.display()
                ));
            }
            let published = publish_seed(&temporary, runtime_db);
            let _ = fs::remove_file(&temporary);
            match published {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!(
                        "cannot install database {}: {error}",
                        runtime_db.display()
                    ));
                }
            }
        }
        let mut database = Connection::open(runtime_db)
            .map_err(|error| format!("cannot open {}: {error}", runtime_db.display()))?;
        database
            .pragma_update(None, "busy_timeout", 3000)
            .map_err(|error| format!("cannot set database busy timeout: {error}"))?;
        database
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(|error| format!("cannot set database journal mode: {error}"))?;
        database
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|error| format!("cannot enable database foreign keys: {error}"))?;
        migrate_database(&mut database)?;
        check_database(&database)?;
        database
            .execute(
                "INSERT INTO domains (id,title,enabled) VALUES (?1,'VRCX',0) \
                 ON CONFLICT(id) DO NOTHING",
                [VRCX_DOMAIN_ID],
            )
            .map_err(|error| format!("cannot register VRCX domain: {error}"))?;
        let snapshot = read_snapshot(&database)?;
        let projection = Arc::new(project(&snapshot)?);
        Ok(Self {
            state: Arc::new(Mutex::new(StoreState {
                database,
                snapshot,
                projection,
            })),
        })
    }

    pub fn snapshot(&self) -> Result<GraphSnapshot, String> {
        Ok(self
            .state
            .lock()
            .map_err(|_| "graph store lock is poisoned".to_owned())?
            .snapshot
            .clone())
    }

    pub(crate) fn projection(&self) -> Result<Arc<GraphProjection>, String> {
        Ok(self
            .state
            .lock()
            .map_err(|_| "graph store lock is poisoned".to_owned())?
            .projection
            .clone())
    }

    pub fn upsert_domain(&self, domain: &GraphDomain) -> Result<(), String> {
        check_id(&domain.id, "domain ID")?;
        check_title(&domain.title, "domain title")?;
        self.edit(|transaction| {
            transaction
                .execute(
                    "INSERT INTO domains (id,title,enabled) VALUES (?1,?2,?3) \
                     ON CONFLICT(id) DO UPDATE SET title=excluded.title, enabled=excluded.enabled",
                    params![domain.id, domain.title, domain.enabled],
                )
                .map_err(|error| format!("cannot save domain {}: {error}", domain.id))?;
            Ok(())
        })
    }

    pub fn upsert_node(&self, node: &GraphNode) -> Result<(), String> {
        check_id(&node.id, "node ID")?;
        check_id(&node.domain_id, "node domain ID")?;
        check_id(&node.subdomain, "node subdomain")?;
        check_title(&node.title, "node title")?;
        if node.activation == CorpusActivation::RuntimeOnly {
            return Err("runtime-only activation is reserved for transient providers".into());
        }
        CorpusTerm {
            ordered_values: node.values.clone(),
        }
        .validate("graph node")?;
        {
            let state = self
                .state
                .lock()
                .map_err(|_| "graph store lock is poisoned".to_owned())?;
            if let Ok(index) = state
                .snapshot
                .nodes
                .binary_search_by(|existing| existing.id.cmp(&node.id))
                && state.snapshot.nodes[index] == *node
            {
                return Ok(());
            }
        }
        let values = serde_json::to_string(&node.values)
            .map_err(|error| format!("cannot encode node values: {error}"))?;
        self.edit(|transaction| {
            transaction
                .execute(
                    "INSERT INTO nodes \
                     (id,domain_id,subdomain,title,enabled,promptable,activation,priority,values_json) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) \
                     ON CONFLICT(id) DO UPDATE SET \
                     domain_id=excluded.domain_id,subdomain=excluded.subdomain,title=excluded.title, \
                     enabled=excluded.enabled,promptable=excluded.promptable, \
                     activation=excluded.activation,priority=excluded.priority, \
                     values_json=excluded.values_json",
                    params![
                        node.id,
                        node.domain_id,
                        node.subdomain,
                        node.title,
                        node.enabled,
                        node.promptable,
                        activation_name(node.activation),
                        node.priority,
                        values,
                    ],
                )
                .map_err(|error| format!("cannot save node {}: {error}", node.id))?;
            Ok(())
        })
    }

    pub fn patch_node_state(&self, patch: &GraphNodeStatePatch) -> Result<(), String> {
        if patch.ids.is_empty() {
            return Err("node state patch needs at least one ID".into());
        }
        if patch.enabled.is_none() && patch.domain_id.is_none() {
            return Err("node state patch has no changes".into());
        }
        if let Some(domain_id) = &patch.domain_id {
            check_id(domain_id, "node domain ID")?;
        }
        let mut seen = HashSet::with_capacity(patch.ids.len());
        for id in &patch.ids {
            check_id(id, "node ID")?;
            if !seen.insert(id.as_str()) {
                return Err(format!("duplicate node ID {id}"));
            }
        }
        self.edit(|transaction| {
            if let Some(domain_id) = &patch.domain_id {
                let exists: i64 = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM domains WHERE id=?1)",
                        [domain_id],
                        |row| row.get(0),
                    )
                    .map_err(|error| format!("cannot inspect domain {domain_id}: {error}"))?;
                if exists == 0 {
                    return Err(format!("unknown graph domain {domain_id}"));
                }
            }
            for id in &patch.ids {
                let updated = match (patch.enabled, patch.domain_id.as_deref()) {
                    (Some(enabled), Some(domain_id)) => transaction.execute(
                        "UPDATE nodes SET enabled=?1,domain_id=?2 WHERE id=?3",
                        params![enabled, domain_id, id],
                    ),
                    (Some(enabled), None) => transaction.execute(
                        "UPDATE nodes SET enabled=?1 WHERE id=?2",
                        params![enabled, id],
                    ),
                    (None, Some(domain_id)) => transaction.execute(
                        "UPDATE nodes SET domain_id=?1 WHERE id=?2",
                        params![domain_id, id],
                    ),
                    (None, None) => unreachable!(),
                }
                .map_err(|error| format!("cannot update node {id}: {error}"))?;
                if updated != 1 {
                    return Err(format!("unknown graph node {id}"));
                }
            }
            Ok(())
        })
    }

    pub fn upsert_edge(&self, edge: &GraphEdge) -> Result<(), String> {
        check_id(&edge.source_id, "edge source ID")?;
        check_id(&edge.target_id, "edge target ID")?;
        self.edit(|transaction| {
            transaction
                .execute(
                    "INSERT INTO edges (source_id,target_id,kind,enabled) VALUES (?1,?2,?3,?4) \
                     ON CONFLICT(source_id,target_id,kind) DO UPDATE SET enabled=excluded.enabled",
                    params![
                        edge.source_id,
                        edge.target_id,
                        edge.kind.as_str(),
                        edge.enabled
                    ],
                )
                .map_err(|error| format!("cannot save edge: {error}"))?;
            Ok(())
        })
    }

    pub fn delete_domain(&self, id: &str) -> Result<(), String> {
        if id == VRCX_DOMAIN_ID {
            return Err("VRCX runtime domain cannot be deleted".into());
        }
        self.edit(|transaction| {
            let count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM nodes WHERE domain_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .map_err(|error| format!("cannot inspect domain {id}: {error}"))?;
            if count != 0 {
                return Err(format!("domain {id} still contains {count} nodes"));
            }
            transaction
                .execute("DELETE FROM domains WHERE id=?1", [id])
                .map_err(|error| format!("cannot delete domain {id}: {error}"))?;
            Ok(())
        })
    }

    pub fn delete_node(&self, id: &str) -> Result<(), String> {
        self.edit(|transaction| {
            transaction
                .execute("DELETE FROM nodes WHERE id=?1", [id])
                .map_err(|error| format!("cannot delete node {id}: {error}"))?;
            Ok(())
        })
    }

    pub fn delete_edge(
        &self,
        source_id: &str,
        target_id: &str,
        kind: GraphEdgeKind,
    ) -> Result<(), String> {
        self.edit(|transaction| {
            transaction
                .execute(
                    "DELETE FROM edges WHERE source_id=?1 AND target_id=?2 AND kind=?3",
                    params![source_id, target_id, kind.as_str()],
                )
                .map_err(|error| format!("cannot delete edge: {error}"))?;
            Ok(())
        })
    }

    fn edit(
        &self,
        action: impl FnOnce(&Transaction<'_>) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "graph store lock is poisoned".to_owned())?;
        let previous = state.projection.clone();
        let transaction = state
            .database
            .transaction()
            .map_err(|error| format!("cannot start graph edit: {error}"))?;
        action(&transaction)?;
        let snapshot = read_snapshot(&transaction)?;
        let mut projection = project(&snapshot)?;
        projection.revision = previous.revision
            + u64::from(
                projection.corpora != previous.corpora
                    || projection.disabled_domains != previous.disabled_domains,
            );
        transaction
            .commit()
            .map_err(|error| format!("cannot commit graph edit: {error}"))?;
        state.snapshot = snapshot;
        state.projection = Arc::new(projection);
        Ok(())
    }
}

fn publish_seed(temporary: &Path, runtime_db: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::iter::once;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::MoveFileW;

        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(once(0)).collect();
        let target: Vec<u16> = runtime_db
            .as_os_str()
            .encode_wide()
            .chain(once(0))
            .collect();
        if unsafe { MoveFileW(source.as_ptr(), target.as_ptr()) } != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(windows))]
    {
        fs::hard_link(temporary, runtime_db).or_else(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                Err(error)
            } else {
                publish_seed_without_hard_links(temporary, runtime_db)
            }
        })
    }
}

#[cfg(target_os = "linux")]
fn publish_seed_without_hard_links(temporary: &Path, runtime_db: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, temporary, CWD, runtime_db, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

#[cfg(all(not(windows), not(target_os = "linux")))]
fn publish_seed_without_hard_links(_temporary: &Path, _runtime_db: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-clobber rename is unavailable on this platform",
    ))
}

/// Validates a distributable seed without modifying it.
pub fn validate_seed_database(path: &Path) -> Result<(), String> {
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("cannot open seed database {}: {error}", path.display()))?;
    check_database(&database)?;
    let snapshot = read_snapshot(&database)?;
    project(&snapshot)?;
    Ok(())
}

fn database_version(database: &Connection) -> Result<i64, String> {
    database
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|error| format!("cannot read corpus database version: {error}"))
}

fn migrate_database(database: &mut Connection) -> Result<(), String> {
    match database_version(database)? {
        DATABASE_VERSION => return Ok(()),
        1 | 2 => {}
        version => {
            return Err(format!(
                "unsupported corpus database version {version}; expected {DATABASE_VERSION}"
            ));
        }
    }
    let transaction = database
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| format!("cannot start corpus database migration: {error}"))?;
    let previous_version = database_version(&transaction)?;
    match previous_version {
        DATABASE_VERSION => return Ok(()),
        1 | 2 => {}
        version => {
            return Err(format!(
                "unsupported corpus database version {version}; expected {DATABASE_VERSION}"
            ));
        }
    }
    transaction
        .execute_batch(match previous_version {
            1 => "ALTER TABLE nodes DROP COLUMN x; ALTER TABLE nodes DROP COLUMN y;",
            2 => "DROP TABLE node_positions;",
            _ => unreachable!(),
        })
        .map_err(|error| format!("cannot migrate corpus database: {error}"))?;
    transaction
        .pragma_update(None, "user_version", DATABASE_VERSION)
        .map_err(|error| format!("cannot set corpus database version: {error}"))?;
    check_database(&transaction)?;
    project(&read_snapshot(&transaction)?)?;
    transaction
        .commit()
        .map_err(|error| format!("cannot commit corpus database migration: {error}"))
}

fn check_database(database: &Connection) -> Result<(), String> {
    let version = database_version(database)?;
    if version != DATABASE_VERSION {
        return Err(format!(
            "unsupported corpus database version {version}; expected {DATABASE_VERSION}"
        ));
    }
    let result: String = database
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|error| format!("cannot check corpus database: {error}"))?;
    if result != "ok" {
        return Err(format!("corpus database integrity check failed: {result}"));
    }
    Ok(())
}

fn read_snapshot(database: &Connection) -> Result<GraphSnapshot, String> {
    let mut snapshot = GraphSnapshot::default();
    let mut statement = database
        .prepare("SELECT id,title,enabled FROM domains ORDER BY id")
        .map_err(|error| format!("cannot read domains: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(GraphDomain {
                id: row.get(0)?,
                title: row.get(1)?,
                enabled: row.get(2)?,
            })
        })
        .map_err(|error| format!("cannot query domains: {error}"))?;
    for row in rows {
        snapshot
            .domains
            .push(row.map_err(|error| format!("invalid domain row: {error}"))?);
    }

    let mut statement = database
        .prepare(
            "SELECT id,domain_id,subdomain,title,enabled,promptable,activation,priority,values_json \
             FROM nodes ORDER BY id",
        )
        .map_err(|error| format!("cannot read nodes: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            let activation: String = row.get(6)?;
            let values: String = row.get(8)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
                activation,
                row.get::<_, i32>(7)?,
                values,
            ))
        })
        .map_err(|error| format!("cannot query nodes: {error}"))?;
    for row in rows {
        let (id, domain_id, subdomain, title, enabled, promptable, activation, priority, values) =
            row.map_err(|error| format!("invalid node row: {error}"))?;
        snapshot.nodes.push(GraphNode {
            id,
            domain_id,
            subdomain,
            title,
            enabled,
            promptable,
            activation: parse_activation(&activation)?,
            priority,
            values: serde_json::from_str(&values)
                .map_err(|error| format!("invalid node values: {error}"))?,
        });
    }

    let mut statement = database
        .prepare(
            "SELECT source_id,target_id,kind,enabled FROM edges ORDER BY source_id,target_id,kind",
        )
        .map_err(|error| format!("cannot read edges: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })
        .map_err(|error| format!("cannot query edges: {error}"))?;
    for row in rows {
        let (source_id, target_id, kind, enabled) =
            row.map_err(|error| format!("invalid edge row: {error}"))?;
        snapshot.edges.push(GraphEdge {
            source_id,
            target_id,
            kind: GraphEdgeKind::parse(&kind)?,
            enabled,
        });
    }
    Ok(snapshot)
}

fn project(snapshot: &GraphSnapshot) -> Result<GraphProjection, String> {
    let mut domains = HashMap::new();
    let mut disabled_domains = HashSet::new();
    for domain in &snapshot.domains {
        check_id(&domain.id, "domain ID")?;
        check_title(&domain.title, "domain title")?;
        if domains.insert(domain.id.as_str(), domain.enabled).is_some() {
            return Err(format!("duplicate domain {}", domain.id));
        }
        if !domain.enabled {
            disabled_domains.insert(domain.id.clone());
        }
    }
    let mut nodes = HashMap::new();
    let mut normalized = HashMap::new();
    for node in &snapshot.nodes {
        check_id(&node.id, "node ID")?;
        check_id(&node.domain_id, "node domain ID")?;
        check_id(&node.subdomain, "node subdomain")?;
        check_title(&node.title, "node title")?;
        if node.activation == CorpusActivation::RuntimeOnly {
            return Err(format!("node {} uses runtime-only activation", node.id));
        }
        CorpusTerm {
            ordered_values: node.values.clone(),
        }
        .validate(&format!("node {}", node.id))?;
        if !domains.contains_key(node.domain_id.as_str()) {
            return Err(format!("node {} has no domain {}", node.id, node.domain_id));
        }
        if nodes.insert(node.id.as_str(), node).is_some() {
            return Err(format!("duplicate node {}", node.id));
        }
        normalized.insert(node.id.as_str(), normalized_values(&node.values));
    }
    let mut inbound: HashMap<&str, (Vec<&GraphNode>, Vec<&GraphNode>, Vec<&GraphNode>)> =
        HashMap::new();
    for edge in &snapshot.edges {
        check_id(&edge.source_id, "edge source ID")?;
        check_id(&edge.target_id, "edge target ID")?;
        if !edge.enabled {
            continue;
        }
        let (Some(source), Some(target)) = (
            nodes.get(edge.source_id.as_str()),
            nodes.get(edge.target_id.as_str()),
        ) else {
            // Unresolved edges are retained for later use when their endpoint
            // is added again.
            continue;
        };
        if !source.enabled
            || !target.enabled
            || !domains[source.domain_id.as_str()]
            || !domains[target.domain_id.as_str()]
        {
            continue;
        }
        let entry = inbound.entry(target.id.as_str()).or_default();
        match edge.kind {
            GraphEdgeKind::Trigger if source.promptable => entry.0.push(source),
            GraphEdgeKind::Trigger => entry.1.push(source),
            GraphEdgeKind::Context => entry.2.push(source),
        }
    }
    struct Group {
        collection: String,
        first_node: String,
        definition: CorpusDefinition,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut group_indexes: HashMap<String, usize> = HashMap::new();
    for node in &snapshot.nodes {
        if !node.enabled || !node.promptable || !domains[node.domain_id.as_str()] {
            continue;
        }
        let (mut triggers, mut trigger_aliases, mut activation_context) =
            inbound.remove(node.id.as_str()).unwrap_or_default();
        dedup_sources(&mut triggers, &normalized);
        dedup_sources(&mut trigger_aliases, &normalized);
        dedup_sources(&mut activation_context, &normalized);
        if node.activation == CorpusActivation::OnEvidence
            && triggers.is_empty()
            && trigger_aliases.is_empty()
        {
            continue;
        }
        let collection = node
            .id
            .rsplit_once(".term-")
            .map_or(node.id.as_str(), |(prefix, _)| prefix)
            .to_owned();
        let signature = serde_json::to_string(&(
            &collection,
            &node.domain_id,
            &node.subdomain,
            &node.title,
            node.priority,
            activation_name(node.activation),
            triggers
                .iter()
                .map(|source| normalized[source.id.as_str()].as_str())
                .collect::<Vec<_>>(),
            trigger_aliases
                .iter()
                .map(|source| normalized[source.id.as_str()].as_str())
                .collect::<Vec<_>>(),
            activation_context
                .iter()
                .map(|source| normalized[source.id.as_str()].as_str())
                .collect::<Vec<_>>(),
        ))
        .map_err(|error| format!("cannot group graph node: {error}"))?;
        if let Some(&index) = group_indexes.get(&signature) {
            groups[index].definition.terms.push(CorpusTerm {
                ordered_values: node.values.clone(),
            });
            continue;
        }
        let definition = CorpusDefinition {
            schema: CORPUS_SCHEMA.into(),
            id: String::new(),
            domain: node.domain_id.clone(),
            subdomain: node.subdomain.clone(),
            title: node.title.clone(),
            priority: node.priority,
            activation: node.activation,
            triggers: triggers
                .iter()
                .map(|source| CorpusTerm {
                    ordered_values: source.values.clone(),
                })
                .collect(),
            trigger_aliases: trigger_aliases
                .iter()
                .map(|source| CorpusTerm {
                    ordered_values: source.values.clone(),
                })
                .collect(),
            activation_context: activation_context
                .iter()
                .map(|source| CorpusTerm {
                    ordered_values: source.values.clone(),
                })
                .collect(),
            terms: vec![CorpusTerm {
                ordered_values: node.values.clone(),
            }],
        };
        group_indexes.insert(signature, groups.len());
        groups.push(Group {
            collection,
            first_node: node.id.clone(),
            definition,
        });
    }
    let mut largest: HashMap<String, (usize, usize)> = HashMap::new();
    for (index, group) in groups.iter().enumerate() {
        let size = group.definition.terms.len();
        let current = largest
            .entry(group.collection.clone())
            .or_insert((index, 0));
        if size > current.1 {
            *current = (index, size);
        }
    }
    let mut corpora = Vec::with_capacity(groups.len());
    let mut used_ids = HashSet::new();
    for (index, mut group) in groups.into_iter().enumerate() {
        let preferred = if largest[&group.collection].0 == index {
            group.collection
        } else {
            group.first_node
        };
        let mut id = preferred.clone();
        let mut suffix = 2;
        while !used_ids.insert(id.clone()) {
            id = format!("{preferred}-group-{suffix}");
            suffix += 1;
        }
        group.definition.id = id;
        group.definition.validate()?;
        corpora.push(group.definition);
    }
    Ok(GraphProjection {
        corpora,
        disabled_domains,
        revision: 0,
    })
}

pub(crate) fn check_id(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.chars().count() > 256
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!("invalid {label} {value:?}"));
    }
    Ok(())
}

fn normalized_values(values: &[String]) -> String {
    values
        .iter()
        .map(|value| value.trim().to_lowercase())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn dedup_sources<'a>(sources: &mut Vec<&'a GraphNode>, normalized: &HashMap<&str, String>) {
    sources.sort_by_key(|source| (normalized[source.id.as_str()].as_str(), source.id.as_str()));
    sources.dedup_by(|left, right| normalized[left.id.as_str()] == normalized[right.id.as_str()]);
}

fn check_title(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty()
        || value.contains('\r')
        || value.contains('\n')
        || value.chars().count() > 256
    {
        return Err(format!("invalid {label} {value:?}"));
    }
    Ok(())
}

fn activation_name(value: CorpusActivation) -> &'static str {
    match value {
        CorpusActivation::OnEvidence => "on-evidence",
        CorpusActivation::Always => "always",
        CorpusActivation::RuntimeOnly => "runtime-only",
    }
}

fn parse_activation(value: &str) -> Result<CorpusActivation, String> {
    match value {
        "on-evidence" => Ok(CorpusActivation::OnEvidence),
        "always" => Ok(CorpusActivation::Always),
        "runtime-only" => Ok(CorpusActivation::RuntimeOnly),
        _ => Err(format!("invalid node activation {value:?}")),
    }
}
