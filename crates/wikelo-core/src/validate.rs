//! Reproducible identity of a dataset and semantic checks on one that arrived from
//! someone else's PC.
//!
//! Trust in uploaded game data cannot come from a signature made on an untrusted
//! machine; it comes from being able to **reproduce** the result. The extractor is
//! deterministic, so two players on the same game build produce the same dataset.
//! [`canonical_hash`] names that content independently of machine-specific fields
//! and of vector order, and [`validate_dataset`] refuses datasets that would break the
//! app (dangling item references, unsafe names used in URLs and file names, absurd
//! sizes) and warns about the suspicious ones.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dataset::*;
use crate::history::{contract_fingerprint, version_key};

/// Bumped whenever the canonical form changes (older hashes stop being comparable).
pub const HASH_VERSION: &str = "v1";

pub const MAX_CONTRACTS: usize = 2000;
pub const MAX_ITEMS: usize = 20_000;
pub const MAX_CRAFTING: usize = 5000;
pub const MAX_MISSION_REWARDS: usize = 20_000;
pub const MAX_WARNINGS: usize = 2000;
const MAX_SHORT: usize = 200;
const MAX_TEXT: usize = 8000;
const MAX_ID: usize = 64;

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

// ----- canonical hash ---------------------------------------------------------------

/// `v1:<sha256>` of the dataset's content: same game files + same extractor build give
/// the same hash on any machine. Leaves out what is machine- or run-specific
/// (`generatedAt`, p4k size, channel folder, warnings, provenance, chosen icon files)
/// and sorts every list whose order carries no meaning.
pub fn canonical_hash(ds: &WikeloDataset) -> String {
    let c = canonicalize(ds);
    let bytes = serde_json::to_vec(&c).unwrap_or_default();
    format!("{HASH_VERSION}:{}", sha256_hex(&bytes))
}

fn canonicalize(ds: &WikeloDataset) -> WikeloDataset {
    let mut c = ds.clone();
    c.source.clear();
    c.generated_at.clear();
    c.build.p4k_size = None;
    c.build.channel.clear();
    c.warnings.clear();
    c.extractor = None;
    c.faction.scopes.sort_by(|a, b| a.id.cmp(&b.id));
    c.emporiums.sort_by(|a, b| a.key.cmp(&b.key));
    for e in &mut c.emporiums {
        e.class_names.sort();
    }
    c.contracts.sort_by(|a, b| a.id.cmp(&b.id));
    for ct in &mut c.contracts {
        ct.aliases.sort();
        ct.emporium_keys.sort();
        ct.completion_tags.sort_by(|a, b| a.id.cmp(&b.id));
        ct.prerequisites.required_tags.sort_by(|a, b| a.id.cmp(&b.id));
        ct.prerequisites.excluded_tags.sort_by(|a, b| a.id.cmp(&b.id));
        ct.prerequisites.required_contract_ids.sort();
        ct.offered_in_build = None;
    }
    c.items.sort_by(|a, b| a.id.cmp(&b.id));
    for it in &mut c.items {
        it.image = None;
        it.image_source = None;
    }
    c.favor_conversions.sort_by(|a, b| a.contract_id.cmp(&b.contract_id));
    c.crafting.sort_by(|a, b| a.blueprint_id.cmp(&b.blueprint_id));
    for r in &mut c.crafting {
        r.sources.sort_by(|a, b| (&a.contract_id, &a.when).cmp(&(&b.contract_id, &b.when)));
    }
    c.mission_rewards.sort_by(|a, b| (&a.contract_id, &a.item_id).cmp(&(&b.contract_id, &b.item_id)));
    c
}

// ----- validation -------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub contracts: usize,
    pub items: usize,
    pub with_image: usize,
    pub crafting: usize,
    pub mission_rewards: usize,
    pub warnings: usize,
}

/// Outcome of [`validate_dataset`]: any error means "do not install".
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub stats: Stats,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// URL-segment safe and free of path tricks: letters, digits, `_ - . :` (resources are
/// `ResourceType:Gold`), no `..`, at most 200 chars. Files named after a class go through
/// the stricter `media::safe_name`, which also refuses `:`.
pub fn safe_class_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 200 && !s.contains("..") && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

fn id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn version_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    (2..=4).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.len() <= 8 && p.chars().all(|c| c.is_ascii_digit()))
}

fn channel_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 20 && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
}

/// `img/<stem>.png|svg` with a file-name-safe stem.
pub fn image_ref_ok(s: &str) -> bool {
    let Some(file) = s.strip_prefix("img/") else { return false };
    let Some((stem, ext)) = file.rsplit_once('.') else { return false };
    matches!(ext, "png" | "svg") && !stem.is_empty() && stem.len() <= 200 && stem.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

fn text_problem(s: &str, max: usize) -> Option<String> {
    if s.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        return Some("contém caracteres de controle".into());
    }
    let n = s.chars().count();
    if n > max {
        return Some(format!("texto com {n} caracteres (máximo {max})"));
    }
    None
}

struct Checker<'a> {
    report: Report,
    items: HashSet<&'a str>,
    contracts: HashSet<&'a str>,
    scopes: HashSet<&'a str>,
    emporiums: HashSet<&'a str>,
}

impl Checker<'_> {
    fn err(&mut self, msg: impl Into<String>) {
        if self.report.errors.len() < 50 {
            self.report.errors.push(msg.into());
        }
    }

    fn warn(&mut self, msg: impl Into<String>) {
        if self.report.warnings.len() < 200 {
            self.report.warnings.push(msg.into());
        }
    }

    fn text(&mut self, what: &str, s: &str, max: usize) {
        if let Some(p) = text_problem(s, max) {
            self.err(format!("{what}: {p}"));
        }
    }

    fn item_ref(&mut self, what: &str, id: &Option<String>, hard: bool) {
        if let Some(id) = id
            && !self.items.contains(id.as_str())
        {
            let msg = format!("{what} referencia o item {id}, que não está na lista de itens");
            if hard { self.err(msg) } else { self.warn(msg) }
        }
    }

    fn requirement(&mut self, where_: &str, r: &Requirement) {
        self.text(&format!("{where_} nome"), &r.name, MAX_SHORT);
        if r.qty < 0 || r.qty_max < r.qty {
            self.err(format!("{where_}: quantidade inválida ({}..{})", r.qty, r.qty_max));
        }
        self.item_ref(where_, &r.item_id, true);
        for c in &r.candidates {
            self.item_ref(&format!("{where_} candidato"), &c.item_id, true);
        }
        for (oi, option) in r.options.iter().enumerate() {
            for (ri, x) in option.iter().enumerate() {
                self.requirement(&format!("{where_} opção {oi} #{ri}"), x);
            }
        }
    }
}

/// Check a dataset before installing it; `previous` is the dataset currently active for
/// the channel (for "this looks like a step backwards" warnings).
pub fn validate_dataset(ds: &WikeloDataset, previous: Option<&WikeloDataset>) -> Report {
    let mut ck = Checker {
        report: Report::default(),
        items: ds.items.iter().map(|i| i.id.as_str()).collect(),
        contracts: ds.contracts.iter().map(|c| c.id.as_str()).collect(),
        scopes: ds.faction.scopes.iter().map(|s| s.id.as_str()).collect(),
        emporiums: ds.emporiums.iter().map(|e| e.key.as_str()).collect(),
    };

    // Header.
    if ds.schema_version != SCHEMA_VERSION {
        ck.err(format!("schemaVersion {} (este servidor entende {})", ds.schema_version, SCHEMA_VERSION));
    }
    if !channel_ok(&ds.build.channel) {
        ck.err(format!("canal inválido: {:?}", ds.build.channel));
    }
    if !version_ok(&ds.build.version) {
        ck.err(format!("versão inválida: {:?}", ds.build.version));
    }
    if !ds.build.p4_change.is_empty() && !ds.build.p4_change.chars().all(|c| c.is_ascii_digit()) {
        ck.err(format!("p4Change inválido: {:?}", ds.build.p4_change));
    }
    for (what, s) in [("branch", &ds.build.branch), ("buildId", &ds.build.build_id), ("tag", &ds.build.tag), ("generatedAt", &ds.generated_at), ("source", &ds.source)] {
        if s.len() > 80 || text_problem(s, 80).is_some() {
            ck.err(format!("{what} inválido"));
        }
    }
    if let Some(v) = ds.build.dcb_version
        && !matches!(v, 6 | 8)
    {
        ck.warn(format!("DataCore v{v} desconhecido"));
    }
    if let Some(x) = &ds.extractor {
        for (what, s) in [("extractor.name", &x.name), ("extractor.version", &x.version), ("extractor.commit", &x.commit), ("extractor.dcbSha256", &x.dcb_sha256), ("extractor.iniSha256", &x.ini_sha256)] {
            if s.len() > 80 || text_problem(s, 80).is_some() {
                ck.err(format!("{what} inválido"));
            }
        }
    }

    // Sizes.
    if ds.contracts.is_empty() {
        ck.err("nenhum contrato");
    }
    for (what, n, max) in [
        ("contratos", ds.contracts.len(), MAX_CONTRACTS),
        ("itens", ds.items.len(), MAX_ITEMS),
        ("receitas", ds.crafting.len(), MAX_CRAFTING),
        ("recompensas de missão", ds.mission_rewards.len(), MAX_MISSION_REWARDS),
        ("avisos", ds.warnings.len(), MAX_WARNINGS),
    ] {
        if n > max {
            ck.err(format!("{what}: {n} (máximo {max})"));
        }
    }
    for w in ds.warnings.iter().take(MAX_WARNINGS) {
        if text_problem(w, 500).is_some() {
            ck.err("aviso com texto inválido");
            break;
        }
    }

    // Faction / reputation scopes.
    if ds.faction.id != WIKELO_FACTION_ID {
        ck.warn(format!("facção {} em vez da do Wikelo", ds.faction.id));
    }
    ck.text("nome da facção", &ds.faction.name, MAX_SHORT);
    let mut seen = HashSet::new();
    for sc in &ds.faction.scopes {
        if !id_ok(&sc.id) || !seen.insert(sc.id.as_str()) {
            ck.err(format!("escopo de reputação com id inválido ou repetido: {:?}", sc.id));
        }
        ck.text("escopo", &sc.name, MAX_SHORT);
        ck.text("escopo", &sc.display_name, MAX_SHORT);
        if sc.ceiling.is_some_and(|c| c < 0) {
            ck.warn(format!("escopo {} com teto negativo", sc.name));
        }
        if sc.standings.windows(2).any(|w| w[0].min_rep > w[1].min_rep) {
            ck.warn(format!("escopo {}: standings fora de ordem", sc.name));
        }
        for st in &sc.standings {
            ck.text("standing", &st.name, MAX_SHORT);
            ck.text("standing", &st.display_name, MAX_SHORT);
        }
    }

    // Emporiums.
    let mut seen = HashSet::new();
    for e in &ds.emporiums {
        if !safe_class_name(&e.key) || !seen.insert(e.key.as_str()) {
            ck.err(format!("emporium com chave inválida ou repetida: {:?}", e.key));
        }
        ck.text("emporium", &e.name, MAX_SHORT);
        for c in &e.class_names {
            ck.text("emporium classe", c, MAX_SHORT);
        }
    }

    // Items first: contracts reference them.
    let mut seen = HashSet::new();
    let mut seen_class = HashSet::new();
    for it in &ds.items {
        if !id_ok(&it.id) || !seen.insert(it.id.as_str()) {
            ck.err(format!("item com id inválido ou repetido: {:?}", it.id));
        }
        if !safe_class_name(&it.class_name) {
            ck.err(format!("item {} com nome de classe inválido: {:?}", it.id, it.class_name));
        } else if !seen_class.insert(it.class_name.as_str()) {
            ck.warn(format!("classe de item repetida: {}", it.class_name));
        }
        ck.text(&format!("item {}", it.id), &it.name, MAX_SHORT);
        for s in [&it.item_type, &it.sub_type].into_iter().flatten() {
            ck.text("tipo de item", s, MAX_SHORT);
        }
        if !matches!(it.kind.as_str(), "currency" | "favour" | "commodity" | "harvestable" | "part" | "armor" | "weapon" | "vehicle" | "component" | "blueprint" | "other") {
            ck.warn(format!("item {} com kind desconhecido: {:?}", it.name, it.kind));
        }
        if let Some(img) = &it.image
            && !image_ref_ok(img)
        {
            ck.err(format!("item {} com referência de imagem inválida: {:?}", it.name, img));
        }
        if let Some(src) = &it.image_source
            && !matches!(src.as_str(), "vehicle_icon" | "frontend_ship" | "loadout_icon" | "inv_icon" | "generic")
        {
            ck.warn(format!("item {} com imageSource desconhecido: {:?}", it.name, src));
        }
    }

    // Contracts.
    let mut seen = HashSet::new();
    let mut seen_debug = HashSet::new();
    for c in &ds.contracts {
        if !id_ok(&c.id) || !seen.insert(c.id.as_str()) {
            ck.err(format!("contrato com id inválido ou repetido: {:?}", c.id));
        }
        let label = if c.title.is_empty() { c.debug_name.clone() } else { c.title.clone() };
        ck.text(&format!("contrato {label} debugName"), &c.debug_name, MAX_SHORT);
        if !c.debug_name.is_empty() && !seen_debug.insert(c.debug_name.as_str()) {
            ck.warn(format!("debugName repetido: {}", c.debug_name));
        }
        ck.text(&format!("contrato {label} título"), &c.title, MAX_SHORT);
        ck.text(&format!("contrato {label} descrição"), &c.description, MAX_TEXT);
        ck.text(&format!("contrato {label} descrição"), &c.description_plain, MAX_TEXT);
        for s in [&c.mission_type, &c.title_key, &c.description_key].into_iter().flatten() {
            ck.text(&format!("contrato {label}"), s, MAX_SHORT);
        }
        for a in &c.aliases {
            ck.text(&format!("contrato {label} alias"), a, MAX_SHORT);
        }
        if c.cooldown.abandoned_sec < 0 || c.cooldown.personal_sec < 0 {
            ck.warn(format!("contrato {label} com cooldown negativo"));
        }
        for v in [c.lifetime.instance_lifetime, c.lifetime.respawn_time].into_iter().flatten() {
            if !v.is_finite() || v < 0.0 {
                ck.warn(format!("contrato {label} com tempo de vida inválido"));
            }
        }
        if let Some(rep) = &c.rep_required {
            if let Some(sid) = &rep.scope_id
                && !ck.scopes.contains(sid.as_str())
            {
                ck.warn(format!("contrato {label} exige reputação num escopo desconhecido"));
            }
            for s in [&rep.min_standing_name, &rep.max_standing_name].into_iter().flatten() {
                ck.text(&format!("contrato {label} standing"), s, MAX_SHORT);
            }
        }
        for g in &c.rep_gained {
            if let Some(sid) = &g.scope_id
                && !ck.scopes.contains(sid.as_str())
            {
                ck.warn(format!("contrato {label} dá reputação num escopo desconhecido"));
            }
        }
        for t in c.completion_tags.iter().chain(&c.prerequisites.required_tags).chain(&c.prerequisites.excluded_tags) {
            if !id_ok(&t.id) {
                ck.err(format!("contrato {label} com tag inválida: {:?}", t.id));
            }
            if let Some(n) = &t.name {
                ck.text("tag", n, MAX_SHORT);
            }
        }
        for req in &c.prerequisites.required_contract_ids {
            if !ck.contracts.contains(req.as_str()) {
                ck.warn(format!("contrato {label} exige o contrato {req}, que não existe"));
            }
        }
        for k in &c.emporium_keys {
            if !ck.emporiums.contains(k.as_str()) {
                ck.warn(format!("contrato {label} aponta para o emporium {k}, que não existe"));
            }
        }
        for (i, r) in c.requirements.iter().enumerate() {
            ck.requirement(&format!("contrato {label} requisito #{i}"), r);
        }
        for (i, r) in c.rewards.iter().enumerate() {
            let where_ = format!("contrato {label} recompensa #{i}");
            ck.text(&where_, &r.name, MAX_SHORT);
            if r.qty < 0 || !r.weight.is_finite() || r.weight < 0.0 {
                ck.err(format!("{where_}: quantidade ou peso inválido"));
            }
            ck.item_ref(&where_, &r.item_id, true);
        }
    }

    // Favors, crafting, mission rewards.
    for f in &ds.favor_conversions {
        if !ck.contracts.contains(f.contract_id.as_str()) {
            ck.warn(format!("conversão de favor aponta para o contrato {}, que não existe", f.contract_id));
        }
        for q in f.give.iter().chain(&f.receive) {
            ck.text("conversão", &q.name, MAX_SHORT);
            if q.qty < 0 {
                ck.err("conversão de favor com quantidade negativa");
            }
            ck.item_ref("conversão de favor", &q.item_id, false);
        }
    }
    let mut seen = HashSet::new();
    for r in &ds.crafting {
        if !id_ok(&r.blueprint_id) || !seen.insert(r.blueprint_id.as_str()) {
            ck.err(format!("receita com id inválido ou repetido: {:?}", r.blueprint_id));
        }
        for s in [&r.blueprint_name, &r.output_name, &r.output_class_name] {
            ck.text(&format!("receita {}", r.blueprint_name), s, MAX_SHORT);
        }
        if !r.output_class_name.is_empty() && !safe_class_name(&r.output_class_name) {
            ck.err(format!("receita {} com classe de saída inválida", r.blueprint_name));
        }
        if let Some(cat) = &r.category {
            ck.text("categoria de receita", cat, MAX_SHORT);
        }
        if !r.craft_time_sec.is_finite() || r.craft_time_sec < 0.0 {
            ck.err(format!("receita {} com tempo inválido", r.blueprint_name));
        }
        if !ck.items.contains(r.output_item_id.as_str()) {
            ck.warn(format!("receita {} produz um item desconhecido", r.blueprint_name));
        }
        for slot in &r.slots {
            ck.text("slot de receita", &slot.name, MAX_SHORT);
            if slot.count == 0 {
                ck.warn(format!("receita {}: slot {} pede 0 opções", r.blueprint_name, slot.name));
            }
            for inp in &slot.options {
                ck.text("ingrediente", &inp.name, MAX_SHORT);
                if !inp.qty.is_finite() || inp.qty < 0.0 || inp.min_quality < 0 {
                    ck.err(format!("receita {}: ingrediente {} com quantidade inválida", r.blueprint_name, inp.name));
                }
                ck.item_ref("ingrediente", &inp.item_id, false);
            }
        }
        for s in &r.sources {
            ck.text("origem de receita", &s.contract_title, MAX_SHORT);
            if !s.chance.is_finite() || !(0.0..=1.0).contains(&s.chance) {
                ck.warn(format!("receita {}: chance fora de 0..1", r.blueprint_name));
            }
        }
    }
    let mut seen = HashSet::new();
    for m in &ds.mission_rewards {
        if !id_ok(&m.contract_id) || !id_ok(&m.item_id) {
            ck.err(format!("recompensa de missão com id inválido: {:?}", m.debug_name));
        }
        if !seen.insert((m.contract_id.as_str(), m.item_id.as_str())) {
            ck.warn(format!("recompensa de missão repetida: {} / {}", m.debug_name, m.name));
        }
        for s in [&m.debug_name, &m.generator, &m.name, &m.class_name] {
            ck.text("recompensa de missão", s, MAX_SHORT);
        }
        if m.qty <= 0 {
            ck.err(format!("recompensa de missão {} com quantidade {}", m.debug_name, m.qty));
        }
        if !ck.items.contains(m.item_id.as_str()) {
            ck.err(format!("recompensa de missão {} referencia o item {}, que não está na lista", m.debug_name, m.item_id));
        }
    }

    // Against what is active now.
    if let Some(prev) = previous {
        let (vn, vp) = (version_key(&ds.build.version), version_key(&prev.build.version));
        if vn < vp {
            ck.warn(format!("versão {} é mais antiga que a ativa ({})", ds.build.version, prev.build.version));
        } else if vn == vp && canonical_hash(ds) != canonical_hash(prev) {
            ck.warn(format!("mesma versão da ativa ({}) com conteúdo diferente", prev.build.version));
        }
        let new_ids: HashSet<&str> = ds.contracts.iter().map(|c| c.id.as_str()).collect();
        let removed: Vec<&Contract> = prev.contracts.iter().filter(|c| !new_ids.contains(c.id.as_str())).collect();
        if !prev.contracts.is_empty() && removed.len() * 10 > prev.contracts.len() * 3 {
            let names: Vec<&str> = removed.iter().take(10).map(|c| if c.title.is_empty() { c.debug_name.as_str() } else { c.title.as_str() }).collect();
            ck.warn(format!("mudança grande: {} dos {} contratos ativos não existem mais ({})", removed.len(), prev.contracts.len(), names.join(", ")));
        }
        let new_items: HashSet<&str> = ds.items.iter().map(|i| i.id.as_str()).collect();
        let removed_items = prev.items.iter().filter(|i| !new_items.contains(i.id.as_str())).count();
        if !prev.items.is_empty() && removed_items * 2 > prev.items.len() {
            ck.warn(format!("mudança grande: {} dos {} itens ativos não existem mais", removed_items, prev.items.len()));
        }
        let prev_fp: HashMap<&str, String> = prev.contracts.iter().map(|c| (c.id.as_str(), contract_fingerprint(c))).collect();
        let changed = ds.contracts.iter().filter(|c| prev_fp.get(c.id.as_str()).is_some_and(|fp| *fp != contract_fingerprint(c))).count();
        if changed * 2 > ds.contracts.len().max(1) {
            ck.warn(format!("mudança grande: {changed} de {} contratos mudaram requisitos ou recompensas", ds.contracts.len()));
        }
    }

    ck.report.stats = Stats {
        contracts: ds.contracts.len(),
        items: ds.items.len(),
        with_image: ds.items.iter().filter(|i| i.image.is_some()).count(),
        crafting: ds.crafting.len(),
        mission_rewards: ds.mission_rewards.len(),
        warnings: ds.warnings.len(),
    };
    ck.report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, class: &str) -> Item {
        Item { id: id.into(), class_name: class.into(), name: class.replace('_', " "), kind: "part".into(), image: Some(format!("img/{class}.png")), image_source: Some("inv_icon".into()), ..Default::default() }
    }

    fn sample() -> WikeloDataset {
        let reward = |id: &str| Reward {
            result: 0,
            option: 0,
            weight: 1.0,
            owner_only: true,
            item_id: Some(id.into()),
            class_name: None,
            name: id.into(),
            qty: 1,
            kind: RewardKind::Currency,
            on_accept: false,
        };
        let contract = |id: &str, title: &str, qty: i64| Contract {
            id: id.into(),
            debug_name: format!("TheCollector_{title}"),
            title: title.into(),
            requirements: vec![Requirement { kind: RequirementKind::Entity, item_id: Some("i1".into()), name: "Favor".into(), qty, qty_max: qty, ..Default::default() }],
            rewards: vec![reward("i2")],
            emporium_keys: vec!["dasi".into()],
            ..Default::default()
        };
        WikeloDataset {
            schema_version: SCHEMA_VERSION,
            source: "local-extract".into(),
            build: BuildInfo { channel: "LIVE".into(), branch: "sc-alpha-4.10.2".into(), version: "4.10.197.2878".into(), p4_change: "12913470".into(), dcb_version: Some(8), p4k_size: Some(1), ..Default::default() },
            generated_at: "2026-10-15T00:00:00Z".into(),
            faction: Faction {
                id: WIKELO_FACTION_ID.into(),
                name: "Wikelo Emporium".into(),
                scopes: vec![RepScope { id: "s2".into(), name: "b".into(), ..Default::default() }, RepScope { id: "s1".into(), name: "a".into(), ..Default::default() }],
            },
            emporiums: vec![Emporium { key: "dasi".into(), name: "Dasi".into(), location_hash: None, class_names: vec!["X".into()] }],
            contracts: vec![contract("c2", "Scorpius", 15), contract("c1", "Intro", 1)],
            items: vec![item("i2", "MG_Scrip"), item("i1", "Wikelo_Favor")],
            favor_conversions: vec![],
            crafting: vec![],
            warnings: vec!["w".into()],
            mission_rewards: vec![MissionReward { contract_id: "m1".into(), debug_name: "Patrol".into(), generator: "HH".into(), item_id: "i2".into(), class_name: "MG_Scrip".into(), name: "MG Scrip".into(), qty: 10, owner_only: true, one_of: false }],
            extractor: Some(ExtractorInfo { name: "wikelo-watcher".into(), version: "0.1.0".into(), commit: "dev".into(), dcb_sha256: "a".into(), ini_sha256: "b".into() }),
        }
    }

    #[test]
    fn hash_ignores_order_and_machine_fields() {
        let a = sample();
        let mut b = sample();
        b.contracts.reverse();
        b.items.reverse();
        b.faction.scopes.reverse();
        b.generated_at = "2030-01-01T00:00:00Z".into();
        b.build.channel = "PTU".into();
        b.build.p4k_size = Some(999);
        b.warnings.clear();
        b.extractor = None;
        for it in &mut b.items {
            it.image = Some("img/other.svg".into());
        }
        assert_eq!(canonical_hash(&a), canonical_hash(&b));
        assert!(canonical_hash(&a).starts_with("v1:"));
        let mut c = sample();
        c.contracts[0].requirements[0].qty += 1;
        assert_ne!(canonical_hash(&a), canonical_hash(&c));
        let mut d = sample();
        d.build.version = "4.11.0.1".into();
        assert_ne!(canonical_hash(&a), canonical_hash(&d));
    }

    #[test]
    fn valid_sample_passes() {
        let r = validate_dataset(&sample(), None);
        assert!(r.ok(), "{:?}", r.errors);
        assert_eq!(r.stats.contracts, 2);
        assert_eq!(r.stats.with_image, 2);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn dangling_and_unsafe_values_are_errors() {
        let mut ds = sample();
        ds.contracts[0].rewards[0].item_id = Some("nope".into());
        let r = validate_dataset(&ds, None);
        assert!(r.errors.iter().any(|e| e.contains("item nope")), "{:?}", r.errors);

        let mut ds = sample();
        ds.items[0].class_name = "../etc/passwd".into();
        assert!(validate_dataset(&ds, None).errors.iter().any(|e| e.contains("classe inválido")));
        let mut ds = sample();
        ds.items[0].class_name = "ResourceType:Gold".into();
        assert!(validate_dataset(&ds, None).ok(), "resources carry a colon");

        let mut ds = sample();
        ds.items[0].image = Some("../../x.png".into());
        assert!(validate_dataset(&ds, None).errors.iter().any(|e| e.contains("imagem inválida")));

        let mut ds = sample();
        ds.contracts[0].title = "a\u{0}b".into();
        assert!(validate_dataset(&ds, None).errors.iter().any(|e| e.contains("controle")));

        let mut ds = sample();
        ds.build.version = "4.10; DROP".into();
        assert!(validate_dataset(&ds, None).errors.iter().any(|e| e.contains("versão inválida")));

        let mut ds = sample();
        ds.contracts.clear();
        assert!(!validate_dataset(&ds, None).ok());
    }

    #[test]
    fn step_backwards_is_a_warning() {
        let active = sample();
        let mut older = sample();
        older.build.version = "4.9.0.1".into();
        older.contracts.truncate(1);
        let r = validate_dataset(&older, Some(&active));
        assert!(r.ok());
        assert!(r.warnings.iter().any(|w| w.contains("mais antiga")), "{:?}", r.warnings);
        assert!(r.warnings.iter().any(|w| w.contains("mudança grande")), "{:?}", r.warnings);
        assert!(validate_dataset(&sample(), Some(&active)).warnings.is_empty());
    }
}
