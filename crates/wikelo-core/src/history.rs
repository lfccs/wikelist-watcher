//! Version history of the dataset: which contracts / items first appeared in
//! which game build, and what the "novidades" of the latest build are.
//!
//! The store keeps one row per (kind, key, version) across every channel that
//! was ever extracted (LIVE, PTU, ...). The computation here is pure so it can
//! be unit tested without SQLite.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::dataset::{Contract, WikeloDataset};

/// `"4.10.197.2878"` → `[4, 10, 197, 2878]`. Non-numeric parts become 0.
pub fn version_key(version: &str) -> Vec<u64> {
    version.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0)).collect()
}

/// Human label for a build: `sc-alpha-4.10.2` → `4.10.2`; falls back to the
/// first three components of the version string.
pub fn version_label(branch: &str, version: &str) -> String {
    let b = branch.trim();
    if let Some(rest) = b.rsplit_once("alpha-").map(|(_, r)| r).or_else(|| b.rsplit('-').next())
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
    {
        return rest.to_string();
    }
    version.split('.').take(3).collect::<Vec<_>>().join(".")
}

/// Stable 64-bit FNV-1a hash of what matters for "did this contract change".
pub fn contract_fingerprint(c: &Contract) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(c.title.clone());
    for r in &c.requirements {
        parts.push(format!("R:{}:{}:{}:{:?}", r.name, r.qty, r.qty_max, r.unit));
        for o in &r.options {
            for x in o {
                parts.push(format!("RO:{}:{}", x.name, x.qty));
            }
        }
    }
    for r in &c.rewards {
        parts.push(format!("W:{}:{}:{}:{}:{:?}", r.result, r.option, r.name, r.qty, r.kind));
    }
    for g in rep_gained_sorted(c) {
        parts.push(format!("G:{}:{}", g.0, g.1));
    }
    if let Some(rr) = &c.rep_required {
        parts.push(format!("Q:{:?}", rr.min_rep));
    }
    parts.push(format!("O:{}", c.once_only));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn rep_gained_sorted(c: &Contract) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = c.rep_gained.iter().map(|g| (g.scope_name.clone().unwrap_or_default(), g.amount)).collect();
    v.sort();
    v
}

/// One game build that was extracted at some point.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BuildSeen {
    pub version: String,
    pub label: String,
    pub channel: String,
    pub p4_change: String,
    pub seen_at: String,
    pub contracts: usize,
    pub items: usize,
}

/// One (kind, key) observed in one build.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SeenRow {
    /// `contract` | `item`
    pub kind: String,
    pub key: String,
    pub version: String,
    pub name: String,
    pub fingerprint: String,
    /// JSON [`ContractSnapshot`] for contracts (empty for items / old rows).
    #[serde(default)]
    pub snapshot: String,
}

/// What a contract looked like in one build; enough to explain a change later.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContractSnapshot {
    pub title: String,
    pub requirements: Vec<SnapQty>,
    pub rewards: Vec<SnapQty>,
    pub rep_gained: i64,
    pub min_rep: Option<i64>,
    pub once_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SnapQty {
    pub name: String,
    pub qty: f64,
    pub unit: String,
}

impl From<&Contract> for ContractSnapshot {
    fn from(c: &Contract) -> Self {
        let mut requirements: Vec<SnapQty> = Vec::new();
        for r in &c.requirements {
            if r.options.is_empty() {
                requirements.push(SnapQty { name: r.name.clone(), qty: r.qty as f64, unit: format!("{:?}", r.unit).to_lowercase() });
            } else {
                for (i, opt) in r.options.iter().enumerate() {
                    for x in opt {
                        requirements.push(SnapQty { name: format!("{} (opção {})", x.name, i + 1), qty: x.qty as f64, unit: format!("{:?}", x.unit).to_lowercase() });
                    }
                }
            }
        }
        let rewards = c
            .rewards
            .iter()
            .map(|r| SnapQty { name: r.name.clone(), qty: r.qty as f64, unit: if r.on_accept { "accept".into() } else { "count".into() } })
            .collect();
        ContractSnapshot {
            title: c.title.clone(),
            requirements,
            rewards,
            rep_gained: c.rep_gained.iter().map(|g| g.amount).sum(),
            min_rep: c.rep_required.as_ref().and_then(|r| r.min_rep),
            once_only: c.once_only,
        }
    }
}

/// One human-readable difference between two builds of a contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// `requirement` | `reward` | `rep` | `title` | `other`
    pub kind: String,
    pub text: String,
}

fn fmt_qty(q: f64, unit: &str) -> String {
    let n = if q.fract() == 0.0 { format!("{}", q as i64) } else { format!("{q}") };
    if unit == "scu" { format!("{n} SCU") } else { format!("{n}×") }
}

/// Explain how `cur` differs from `prev`.
pub fn diff_snapshots(prev: &ContractSnapshot, cur: &ContractSnapshot) -> Vec<Change> {
    let mut out = Vec::new();
    if prev.title != cur.title {
        out.push(Change { kind: "title".into(), text: format!("Título: “{}” → “{}”", prev.title, cur.title) });
    }
    let diff_list = |kind: &str, a: &[SnapQty], b: &[SnapQty], out: &mut Vec<Change>| {
        let label_add = if kind == "requirement" { "Passou a pedir" } else { "Nova recompensa:" };
        let label_rm = if kind == "requirement" { "Não pede mais" } else { "Deixou de dar" };
        for x in b {
            match a.iter().find(|y| y.name == x.name) {
                None => out.push(Change { kind: kind.into(), text: format!("{label_add} {} {}", fmt_qty(x.qty, &x.unit), x.name) }),
                Some(y) if (y.qty - x.qty).abs() > 1e-9 => out.push(Change {
                    kind: kind.into(),
                    text: format!("{}: {} → {}", x.name, fmt_qty(y.qty, &y.unit), fmt_qty(x.qty, &x.unit)),
                }),
                _ => {}
            }
        }
        for y in a {
            if !b.iter().any(|x| x.name == y.name) {
                out.push(Change { kind: kind.into(), text: format!("{label_rm} {} {}", fmt_qty(y.qty, &y.unit), y.name) });
            }
        }
    };
    diff_list("requirement", &prev.requirements, &cur.requirements, &mut out);
    diff_list("reward", &prev.rewards, &cur.rewards, &mut out);
    if prev.rep_gained != cur.rep_gained {
        out.push(Change { kind: "rep".into(), text: format!("Reputação: +{} → +{}", prev.rep_gained, cur.rep_gained) });
    }
    if prev.min_rep != cur.min_rep {
        let f = |v: Option<i64>| v.map(|x| x.to_string()).unwrap_or_else(|| "nenhuma".into());
        out.push(Change { kind: "rep".into(), text: format!("Rep mínima: {} → {}", f(prev.min_rep), f(cur.min_rep)) });
    }
    if prev.once_only != cur.once_only {
        out.push(Change { kind: "other".into(), text: if cur.once_only { "Passou a ser única".into() } else { "Deixou de ser única (repetível)".into() } });
    }
    out
}

/// Rows describing one dataset, ready to be recorded.
pub fn dataset_rows(ds: &WikeloDataset) -> (BuildSeen, Vec<SeenRow>) {
    let build = BuildSeen {
        version: ds.build.version.clone(),
        label: version_label(&ds.build.branch, &ds.build.version),
        channel: ds.build.channel.clone(),
        p4_change: ds.build.p4_change.clone(),
        seen_at: ds.generated_at.clone(),
        contracts: ds.contracts.len(),
        items: ds.items.len(),
    };
    let mut rows = Vec::with_capacity(ds.contracts.len() + ds.items.len());
    for c in &ds.contracts {
        rows.push(SeenRow {
            kind: "contract".into(),
            key: c.id.clone(),
            version: ds.build.version.clone(),
            name: c.title.clone(),
            fingerprint: contract_fingerprint(c),
            snapshot: serde_json::to_string(&ContractSnapshot::from(c)).unwrap_or_default(),
        });
    }
    for it in &ds.items {
        rows.push(SeenRow {
            kind: "item".into(),
            key: it.id.clone(),
            version: ds.build.version.clone(),
            name: it.name.clone(),
            fingerprint: it.kind.clone(),
            snapshot: String::new(),
        });
    }
    (build, rows)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedContract {
    pub id: String,
    pub title: String,
    /// Label of the last build where it looked different.
    pub since: String,
    /// What changed; empty when the previous build was recorded without a snapshot.
    #[serde(default)]
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemovedEntry {
    pub kind: String,
    pub key: String,
    pub name: String,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewEntry {
    pub kind: String,
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct WhatsNew {
    pub current_version: String,
    pub current_label: String,
    /// Earliest build ever recorded (what "new" is measured against).
    pub baseline_label: Option<String>,
    /// True once at least two distinct builds are known.
    pub has_history: bool,
    /// Every build recorded, oldest first.
    pub builds: Vec<BuildSeen>,
    /// Label of the most recent build (≤ current) that introduced something.
    pub latest_novelty_label: Option<String>,
    pub latest_novelty_version: Option<String>,
    /// Contract id → label of the build where it first appeared.
    pub first_seen: HashMap<String, String>,
    /// Contracts and items that appeared in `latest_novelty_version`.
    pub new_contracts: Vec<NewEntry>,
    pub new_items: Vec<NewEntry>,
    /// Contracts whose requirements/rewards differ from the previous recorded build.
    pub changed_contracts: Vec<ChangedContract>,
    /// Known before, absent from the current build.
    pub removed: Vec<RemovedEntry>,
}

/// One recorded observation of a key in a build.
#[derive(Clone)]
struct SeenVersion {
    version: String,
    name: String,
    fingerprint: String,
    snapshot: String,
}

/// (kind, key) -> version key -> observation
type SeenMap = HashMap<(String, String), BTreeMap<Vec<u64>, SeenVersion>>;

/// Compute the novelties for `current` (a dataset) from the recorded history.
pub fn compute_whats_new(builds: &[BuildSeen], rows: &[SeenRow], current: &WikeloDataset) -> WhatsNew {
    let cur_key = version_key(&current.build.version);
    let mut builds: Vec<BuildSeen> = builds.to_vec();
    builds.sort_by(|a, b| version_key(&a.version).cmp(&version_key(&b.version)).then_with(|| a.p4_change.cmp(&b.p4_change)));
    let label_of: HashMap<&str, &str> = builds.iter().map(|b| (b.version.as_str(), b.label.as_str())).collect();
    let label = |v: &str| label_of.get(v).map(|s| s.to_string()).unwrap_or_else(|| v.to_string());

    // Only builds up to the one being viewed count (PTU ahead of LIVE must not leak).
    let considered: Vec<&BuildSeen> = builds.iter().filter(|b| version_key(&b.version) <= cur_key).collect();
    let baseline = considered.first().map(|b| b.version.clone());
    let previous = considered.iter().rev().find(|b| version_key(&b.version) < cur_key).map(|b| b.version.clone());

    // (kind,key) -> version key -> (version, name, fingerprint)
    let mut seen: SeenMap = HashMap::new();
    for r in rows {
        let vk = version_key(&r.version);
        if vk > cur_key {
            continue;
        }
        seen.entry((r.kind.clone(), r.key.clone())).or_default().insert(
            vk,
            SeenVersion { version: r.version.clone(), name: r.name.clone(), fingerprint: r.fingerprint.clone(), snapshot: r.snapshot.clone() },
        );
    }

    let mut out = WhatsNew {
        current_version: current.build.version.clone(),
        current_label: version_label(&current.build.branch, &current.build.version),
        baseline_label: baseline.as_deref().map(label),
        has_history: considered.len() >= 2,
        builds: builds.clone(),
        ..Default::default()
    };
    if !out.has_history {
        return out;
    }
    let baseline_key = version_key(baseline.as_deref().unwrap_or_default());

    // First-seen per key and the latest build that introduced a contract.
    let mut first_seen_version: HashMap<(String, String), Vec<u64>> = HashMap::new();
    let mut latest_novelty: Option<Vec<u64>> = None;
    for (k, versions) in &seen {
        let first = versions.keys().next().cloned().unwrap_or_default();
        if first > baseline_key && k.0 == "contract" {
            // Only contracts that still exist in the current build count as novelties.
            if current.contracts.iter().any(|c| c.id == k.1) && latest_novelty.as_ref().is_none_or(|l| first > *l) {
                latest_novelty = Some(first.clone());
            }
        }
        first_seen_version.insert(k.clone(), first);
    }
    for c in &current.contracts {
        if let Some(v) = seen.get(&("contract".to_string(), c.id.clone())).and_then(|m| m.values().next()) {
            out.first_seen.insert(c.id.clone(), label(&v.version));
        }
    }

    if let Some(nv) = latest_novelty {
        let nv_str = seen.values().find_map(|m| m.get(&nv).map(|v| v.version.clone())).unwrap_or_default();
        out.latest_novelty_label = Some(label(&nv_str));
        out.latest_novelty_version = Some(nv_str);
        for c in &current.contracts {
            if first_seen_version.get(&("contract".to_string(), c.id.clone())) == Some(&nv) {
                out.new_contracts.push(NewEntry { kind: "contract".into(), key: c.id.clone(), name: c.title.clone() });
            }
        }
        for it in &current.items {
            if first_seen_version.get(&("item".to_string(), it.id.clone())) == Some(&nv) {
                out.new_items.push(NewEntry { kind: "item".into(), key: it.id.clone(), name: it.name.clone() });
            }
        }
        out.new_contracts.sort_by(|a, b| a.name.cmp(&b.name));
        out.new_items.sort_by(|a, b| a.name.cmp(&b.name));
    }

    // Changed: fingerprint differs from the previous recorded build where it existed.
    for c in &current.contracts {
        let Some(versions) = seen.get(&("contract".to_string(), c.id.clone())) else { continue };
        let now_fp = contract_fingerprint(c);
        if let Some((_, prev)) = versions.iter().rev().find(|(k, _)| **k < cur_key)
            && prev.fingerprint != now_fp
        {
            let changes = serde_json::from_str::<ContractSnapshot>(&prev.snapshot)
                .map(|p| diff_snapshots(&p, &ContractSnapshot::from(c)))
                .unwrap_or_default();
            out.changed_contracts.push(ChangedContract { id: c.id.clone(), title: c.title.clone(), since: label(&prev.version), changes });
        }
    }
    out.changed_contracts.sort_by(|a, b| a.title.cmp(&b.title));

    // Removed: seen before, not in the current dataset.
    let _ = previous;
    let cur_contracts: HashMap<&str, ()> = current.contracts.iter().map(|c| (c.id.as_str(), ())).collect();
    let cur_items: HashMap<&str, ()> = current.items.iter().map(|i| (i.id.as_str(), ())).collect();
    for ((kind, key), versions) in &seen {
        let present = match kind.as_str() {
            "contract" => cur_contracts.contains_key(key.as_str()),
            _ => cur_items.contains_key(key.as_str()),
        };
        if present {
            continue;
        }
        if let Some((_, last)) = versions.iter().next_back() {
            out.removed.push(RemovedEntry { kind: kind.clone(), key: key.clone(), name: last.name.clone(), last_seen: label(&last.version) });
        }
    }
    out.removed.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::*;

    fn ds(version: &str, branch: &str, contracts: &[(&str, &str, i64)], items: &[&str]) -> WikeloDataset {
        WikeloDataset {
            schema_version: 1,
            source: "test".into(),
            build: BuildInfo { channel: "LIVE".into(), branch: branch.into(), version: version.into(), ..Default::default() },
            generated_at: "2026-01-01T00:00:00Z".into(),
            faction: Faction::default(),
            emporiums: vec![],
            contracts: contracts
                .iter()
                .map(|(id, title, qty)| Contract {
                    id: id.to_string(),
                    title: title.to_string(),
                    requirements: vec![Requirement { name: "Favor".into(), qty: *qty, ..Default::default() }],
                    ..Default::default()
                })
                .collect(),
            items: items.iter().map(|i| Item { id: i.to_string(), name: i.to_string(), ..Default::default() }).collect(),
            favor_conversions: vec![],
            crafting: vec![],
            warnings: vec![],
            mission_rewards: vec![],
            extractor: None,
        }
    }

    #[test]
    fn version_helpers() {
        assert_eq!(version_key("4.10.197.2878"), vec![4, 10, 197, 2878]);
        assert!(version_key("4.10.197.2878") > version_key("4.9.185.24066"));
        assert_eq!(version_label("sc-alpha-4.10.2", "4.10.197.2878"), "4.10.2");
        assert_eq!(version_label("", "4.10.197.2878"), "4.10.197");
    }

    #[test]
    fn novelties_between_two_builds() {
        let old = ds("4.9.185.24066", "sc-alpha-4.9.0", &[("a", "A", 1), ("b", "B", 1), ("gone", "Gone", 1)], &["i1"]);
        let new = ds("4.10.197.2878", "sc-alpha-4.10.2", &[("a", "A", 1), ("b", "B", 5), ("c", "C", 1)], &["i1", "i2"]);
        let (b1, r1) = dataset_rows(&old);
        let (b2, r2) = dataset_rows(&new);
        let rows: Vec<SeenRow> = r1.into_iter().chain(r2).collect();
        let wn = compute_whats_new(&[b1.clone(), b2.clone()], &rows, &new);
        assert!(wn.has_history);
        assert_eq!(wn.baseline_label.as_deref(), Some("4.9.0"));
        assert_eq!(wn.latest_novelty_label.as_deref(), Some("4.10.2"));
        assert_eq!(wn.new_contracts.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["c"]);
        assert_eq!(wn.new_items.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(), vec!["i2"]);
        assert_eq!(wn.changed_contracts.len(), 1);
        assert_eq!(wn.changed_contracts[0].id, "b");
        assert_eq!(wn.changed_contracts[0].changes.len(), 1);
        assert_eq!(wn.changed_contracts[0].changes[0].text, "Favor: 1× → 5×");
        assert_eq!(wn.removed.len(), 1);
        assert_eq!(wn.removed[0].key, "gone");
        assert_eq!(wn.first_seen.get("c").map(String::as_str), Some("4.10.2"));
        assert_eq!(wn.first_seen.get("a").map(String::as_str), Some("4.9.0"));

        // A later patch with nothing new keeps pointing at 4.10.2.
        let patch = ds("4.10.250.1", "sc-alpha-4.10.3", &[("a", "A", 1), ("b", "B", 5), ("c", "C", 1)], &["i1", "i2"]);
        let (b3, r3) = dataset_rows(&patch);
        let rows: Vec<SeenRow> = rows.into_iter().chain(r3).collect();
        let wn = compute_whats_new(&[b1.clone(), b2.clone(), b3], &rows, &patch);
        assert_eq!(wn.latest_novelty_label.as_deref(), Some("4.10.2"));
        assert_eq!(wn.new_contracts.len(), 1);
        assert!(wn.changed_contracts.is_empty());

        // Viewing the old build must not see the future.
        let wn_old = compute_whats_new(&[b1, b2], &rows, &old);
        assert!(!wn_old.has_history);
    }
}
