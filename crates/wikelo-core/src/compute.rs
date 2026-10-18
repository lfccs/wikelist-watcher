//! Pure computations over a dataset and the player's inventory.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::dataset::{Contract, CraftRecipe, MissionReward, Requirement, RequirementKind, RewardKind, Unit, WikeloDataset};

/// Player inventory: item id -> quantity (units or SCU, matching the item).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Inventory(pub HashMap<String, i64>);

impl Inventory {
    pub fn qty(&self, item_id: &str) -> i64 {
        self.0.get(item_id).copied().unwrap_or(0)
    }

    pub fn set(&mut self, item_id: impl Into<String>, qty: i64) {
        self.0.insert(item_id.into(), qty.max(0));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequirementStatus {
    pub name: String,
    pub item_id: Option<String>,
    pub need: i64,
    pub have: i64,
    pub missing: i64,
    pub satisfied: bool,
    /// When the item can be crafted: what the recipe needs for the missing units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub craft: Option<CraftStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CraftInputStatus {
    pub name: String,
    pub item_id: Option<String>,
    pub unit: Unit,
    pub min_quality: i64,
    /// Total needed for the units still missing (fractional SCU possible).
    pub need: f64,
    pub have: i64,
    pub missing: f64,
    pub satisfied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CraftSlotStatus {
    pub name: String,
    pub count: u32,
    pub options: Vec<CraftInputStatus>,
    pub satisfied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CraftStatus {
    pub blueprint_id: String,
    /// How many units the recipe status was computed for (the missing count, at least 1).
    pub crafts: i64,
    pub slots: Vec<CraftSlotStatus>,
    pub can_craft: bool,
    /// The blueprint is handed out when a Wikelo contract is accepted: the item
    /// only exists through this craft (no loot or shop alternative).
    pub exclusive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContractStatus {
    pub contract_id: String,
    pub requirements: Vec<RequirementStatus>,
    /// Sum of missing units across requirements (SCU and units mixed, for ranking only).
    pub missing_total: i64,
    /// 0.0 .. 1.0 share of required units already in inventory.
    pub progress: f64,
    pub items_satisfied: bool,
    pub rep_blocked: bool,
    pub can_complete: bool,
}

/// Evaluate a contract against an inventory and the player's reputation in the
/// contract's scope (None = unknown, treated as not blocked).
pub fn evaluate(contract: &Contract, inventory: &Inventory, current_rep: Option<i64>) -> ContractStatus {
    evaluate_with(contract, inventory, current_rep, &[])
}

/// Like [`evaluate`], also reporting crafting options for missing items.
pub fn evaluate_with(contract: &Contract, inventory: &Inventory, current_rep: Option<i64>, recipes: &[CraftRecipe]) -> ContractStatus {
    let statuses: Vec<RequirementStatus> =
        contract.requirements.iter().map(|req| status_for(req, inventory, recipes)).collect();
    let need_total: i64 = statuses.iter().map(|s| s.need).sum();
    let have_total: i64 = statuses.iter().map(|s| s.have.min(s.need)).sum();
    let missing_total: i64 = statuses.iter().map(|s| s.missing).sum();
    let items_satisfied = statuses.iter().all(|s| s.satisfied);
    let rep_blocked = match (current_rep, contract.rep_required.as_ref().and_then(|r| r.min_rep)) {
        (Some(have), Some(min)) => have < min,
        _ => false,
    };
    let progress = if need_total == 0 { 1.0 } else { have_total as f64 / need_total as f64 };
    ContractStatus {
        contract_id: contract.id.clone(),
        requirements: statuses,
        missing_total,
        progress,
        items_satisfied,
        rep_blocked,
        can_complete: items_satisfied && !rep_blocked,
    }
}

/// Recipe status for `crafts` units of the output.
pub fn craft_status(recipe: &CraftRecipe, inventory: &Inventory, crafts: i64) -> CraftStatus {
    let crafts = crafts.max(1);
    let slots: Vec<CraftSlotStatus> = recipe
        .slots
        .iter()
        .map(|slot| {
            let options: Vec<CraftInputStatus> = slot
                .options
                .iter()
                .map(|o| {
                    let need = o.qty * crafts as f64;
                    let have = o.item_id.as_deref().map(|id| inventory.qty(id)).unwrap_or(0);
                    let missing = (need - have as f64).max(0.0);
                    CraftInputStatus {
                        name: o.name.clone(),
                        item_id: o.item_id.clone(),
                        unit: o.unit,
                        min_quality: o.min_quality,
                        need,
                        have,
                        missing,
                        satisfied: missing <= 0.0,
                    }
                })
                .collect();
            let ok = options.iter().filter(|o| o.satisfied).count() as u32 >= slot.count.max(1);
            CraftSlotStatus { name: slot.name.clone(), count: slot.count, options, satisfied: ok }
        })
        .collect();
    let can_craft = !slots.is_empty() && slots.iter().all(|s| s.satisfied);
    let exclusive = recipe.sources.iter().any(|s| s.when == "accept");
    CraftStatus { blueprint_id: recipe.blueprint_id.clone(), crafts, slots, can_craft, exclusive }
}

fn status_for(req: &Requirement, inventory: &Inventory, recipes: &[CraftRecipe]) -> RequirementStatus {
    match req.kind {
        RequirementKind::Or => {
            // Pick the option closest to completion.
            let mut best: Option<RequirementStatus> = None;
            for option in &req.options {
                let parts: Vec<RequirementStatus> = option.iter().map(|r| status_for(r, inventory, recipes)).collect();
                let need: i64 = parts.iter().map(|p| p.need).sum();
                let have: i64 = parts.iter().map(|p| p.have.min(p.need)).sum();
                let missing: i64 = parts.iter().map(|p| p.missing).sum();
                let candidate = RequirementStatus {
                    name: option.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(" + "),
                    item_id: None,
                    need,
                    have,
                    missing,
                    satisfied: missing == 0,
                    craft: None,
                };
                if best.as_ref().is_none_or(|b| candidate.missing < b.missing) {
                    best = Some(candidate);
                }
            }
            best.unwrap_or(RequirementStatus {
                name: req.name.clone(),
                item_id: None,
                need: req.qty,
                have: 0,
                missing: req.qty,
                satisfied: req.qty == 0,
                craft: None,
            })
        }
        RequirementKind::Entities | RequirementKind::MissionItem if !req.candidates.is_empty() => {
            // Any candidate counts; sum what the player has across candidates.
            let have: i64 = req
                .candidates
                .iter()
                .filter_map(|c| c.item_id.as_deref())
                .map(|id| inventory.qty(id))
                .sum::<i64>()
                + req.item_id.as_deref().map(|id| inventory.qty(id)).unwrap_or(0);
            let missing = (req.qty - have).max(0);
            RequirementStatus {
                name: req.name.clone(),
                item_id: req.item_id.clone(),
                need: req.qty,
                have,
                missing,
                satisfied: missing == 0,
                craft: None,
            }
        }
        _ => {
            let have = req.item_id.as_deref().map(|id| inventory.qty(id)).unwrap_or(0);
            let missing = (req.qty - have).max(0);
            let craft = req
                .item_id
                .as_deref()
                .and_then(|id| recipes.iter().find(|r| r.output_item_id == id))
                .map(|r| craft_status(r, inventory, missing));
            RequirementStatus {
                name: req.name.clone(),
                item_id: req.item_id.clone(),
                need: req.qty,
                have,
                missing,
                satisfied: missing == 0,
                craft,
            }
        }
    }
}

/// Evaluate every contract and sort by closeness to completion
/// (completable first, then highest progress, then fewest missing units).
pub fn rank_contracts(
    dataset: &WikeloDataset,
    inventory: &Inventory,
    current_rep: Option<i64>,
) -> Vec<ContractStatus> {
    let mut out: Vec<ContractStatus> =
        dataset.contracts.iter().map(|c| evaluate_with(c, inventory, current_rep, &dataset.crafting)).collect();
    out.sort_by(|a, b| {
        b.can_complete
            .cmp(&a.can_complete)
            .then_with(|| b.progress.partial_cmp(&a.progress).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.missing_total.cmp(&b.missing_total))
    });
    out
}

/// How many contracts use each item, for the inventory page ("used in N contracts").
pub fn item_usage(dataset: &WikeloDataset) -> HashMap<String, usize> {
    let mut map: HashMap<String, usize> = HashMap::new();
    for c in &dataset.contracts {
        let mut seen = HashSet::new();
        for r in &c.requirements {
            collect_item_ids(r, &mut seen);
        }
        for id in seen {
            *map.entry(id).or_insert(0) += 1;
        }
    }
    map
}

fn collect_item_ids(req: &Requirement, out: &mut HashSet<String>) {
    if let Some(id) = &req.item_id {
        out.insert(id.clone());
    }
    for c in &req.candidates {
        if let Some(id) = &c.item_id {
            out.insert(id.clone());
        }
    }
    for opt in &req.options {
        for r in opt {
            collect_item_ids(r, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dataset::*;

    fn req(id: &str, qty: i64) -> Requirement {
        Requirement {
            kind: RequirementKind::Entity,
            item_id: Some(id.into()),
            class_name: None,
            name: id.into(),
            qty,
            qty_max: qty,
            unit: Unit::Count,
            candidates: vec![],
            options: vec![],
        }
    }

    fn contract(reqs: Vec<Requirement>, min_rep: Option<i64>) -> Contract {
        Contract {
            id: "c1".into(),
            debug_name: "TheCollector_Test".into(),
            aliases: vec![],
            category: Category::Other,
            mission_type: None,
            title_key: None,
            title: "t".into(),
            description_key: None,
            description: String::new(),
            description_plain: String::new(),
            rep_required: min_rep.map(|m| RepRequirement { min_rep: Some(m), ..Default::default() }),
            rep_gained: vec![],
            once_only: false,
            not_for_release: false,
            work_in_progress: false,
            hidden_in_mobiglas: false,
            cooldown: Cooldown::default(),
            lifetime: Lifetime::default(),
            prerequisites: Prerequisites::default(),
            completion_tags: vec![],
            emporium_keys: vec![],
            offered_in_build: None,
            requirements: reqs,
            rewards: vec![],
        }
    }

    #[test]
    fn craftable_requirement_reports_materials() {
        let recipe = CraftRecipe {
            blueprint_id: "bp".into(),
            output_item_id: "meta".into(),
            output_name: "Metamaterial".into(),
            slots: vec![
                CraftSlot {
                    name: "Case".into(),
                    count: 1,
                    options: vec![CraftInput { kind: "resource".into(), item_id: Some("stileron".into()), name: "Stileron".into(), qty: 0.5, unit: Unit::Scu, min_quality: 700, ..Default::default() }],
                },
                CraftSlot {
                    name: "Support".into(),
                    count: 1,
                    options: vec![CraftInput { kind: "resource".into(), item_id: Some("ouratite".into()), name: "Ouratite".into(), qty: 0.3, unit: Unit::Scu, ..Default::default() }],
                },
            ],
            ..Default::default()
        };
        let c = contract(vec![req("meta", 1)], None);
        let mut inv = Inventory::default();
        inv.set("stileron", 1);
        let s = evaluate_with(&c, &inv, None, std::slice::from_ref(&recipe));
        let craft = s.requirements[0].craft.as_ref().expect("craft status");
        assert_eq!(craft.crafts, 1);
        assert!(craft.slots[0].satisfied);
        assert!(!craft.slots[1].satisfied);
        assert!(!craft.can_craft);
        inv.set("ouratite", 1);
        let s = evaluate_with(&c, &inv, None, std::slice::from_ref(&recipe));
        assert!(s.requirements[0].craft.as_ref().unwrap().can_craft);
        assert!(!s.requirements[0].craft.as_ref().unwrap().exclusive, "no on-accept source: item has other ways to be obtained");
        assert!(!s.can_complete, "crafting possibility does not satisfy the requirement by itself");
    }

    #[test]
    fn missing_and_progress() {
        let c = contract(vec![req("favor", 15), req("board", 4)], None);
        let mut inv = Inventory::default();
        inv.set("favor", 10);
        inv.set("board", 4);
        let s = evaluate(&c, &inv, None);
        assert_eq!(s.missing_total, 5);
        assert!(!s.can_complete);
        assert!((s.progress - 14.0 / 19.0).abs() < 1e-9);
        inv.set("favor", 20);
        let s = evaluate(&c, &inv, None);
        assert!(s.can_complete);
        assert_eq!(s.progress, 1.0);
    }

    #[test]
    fn rep_gate_blocks() {
        let c = contract(vec![], Some(999));
        let s = evaluate(&c, &Inventory::default(), Some(340));
        assert!(s.rep_blocked);
        assert!(!s.can_complete);
        let s = evaluate(&c, &Inventory::default(), None);
        assert!(s.can_complete);
    }

    #[test]
    fn or_requirement_picks_best_option() {
        let mut or = req("x", 0);
        or.kind = RequirementKind::Or;
        or.options = vec![vec![req("a", 10)], vec![req("b", 2)]];
        let mut inv = Inventory::default();
        inv.set("b", 1);
        let c = contract(vec![or], None);
        let s = evaluate(&c, &inv, None);
        assert_eq!(s.requirements[0].missing, 1);
    }
}

// ----- inventory changes implied by a completed mission --------------------------

/// One inventory delta proposed or applied after a mission completed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InventoryChange {
    pub item_id: String,
    pub name: String,
    pub delta: i64,
}

/// Deltas split by how sure we are. `certain` can be applied without asking: the
/// quantity comes straight from the game data and nothing is ambiguous. `uncertain`
/// covers alternatives (one of several items, random reward pools, quantity ranges)
/// and must be confirmed by the player.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InventoryPlan {
    pub certain: Vec<InventoryChange>,
    pub uncertain: Vec<InventoryChange>,
}

impl InventoryPlan {
    pub fn is_empty(&self) -> bool {
        self.certain.is_empty() && self.uncertain.is_empty()
    }
}

fn stackable(kind: &RewardKind) -> bool {
    matches!(kind, RewardKind::Currency | RewardKind::Favour | RewardKind::Other)
}

/// What completing a Wikelo contract does to the inventory: requirements are consumed,
/// stackable rewards are gained.
pub fn plan_completion(contract: &Contract) -> InventoryPlan {
    let mut plan = InventoryPlan::default();
    for r in &contract.requirements {
        match r.kind {
            RequirementKind::Or => {
                // One of several alternative sets: we cannot tell which one was delivered.
                for opt in &r.options {
                    for q in opt {
                        if let Some(id) = &q.item_id
                            && q.qty > 0
                        {
                            plan.uncertain.push(InventoryChange { item_id: id.clone(), name: q.name.clone(), delta: -q.qty });
                        }
                    }
                }
            }
            RequirementKind::Entities => {
                for c in &r.candidates {
                    if let Some(id) = &c.item_id
                        && r.qty > 0
                    {
                        plan.uncertain.push(InventoryChange { item_id: id.clone(), name: c.name.clone(), delta: -r.qty });
                    }
                }
            }
            _ => {
                if let Some(id) = &r.item_id
                    && r.qty > 0
                {
                    let change = InventoryChange { item_id: id.clone(), name: r.name.clone(), delta: -r.qty };
                    if r.qty_max == r.qty || r.qty_max == 0 { plan.certain.push(change) } else { plan.uncertain.push(change) }
                }
            }
        }
    }
    // Rewards: a `result` block with a single option is certain; weighted pools are not.
    let mut options_per_result: HashMap<u32, HashSet<u32>> = HashMap::new();
    for rw in &contract.rewards {
        options_per_result.entry(rw.result).or_default().insert(rw.option);
    }
    for rw in &contract.rewards {
        if rw.on_accept || !stackable(&rw.kind) || rw.qty <= 0 {
            continue;
        }
        let Some(id) = &rw.item_id else { continue };
        let change = InventoryChange { item_id: id.clone(), name: rw.name.clone(), delta: rw.qty };
        if options_per_result.get(&rw.result).is_some_and(|o| o.len() == 1) {
            plan.certain.push(change);
        } else {
            plan.uncertain.push(change);
        }
    }
    plan
}

/// What completing a non-Wikelo mission pays, from the game data. `owned` says whether
/// this player accepted the mission (owner-only rewards go to nobody else). `one_of` sets
/// are not deltas but a question for the player, so they are left out here.
pub fn plan_mission_rewards(rewards: &[&MissionReward], owned: bool) -> InventoryPlan {
    let mut plan = InventoryPlan::default();
    for r in rewards {
        if r.qty <= 0 || r.one_of {
            continue;
        }
        let change = InventoryChange { item_id: r.item_id.clone(), name: r.name.clone(), delta: r.qty };
        if owned || !r.owner_only { plan.certain.push(change) } else { plan.uncertain.push(change) }
    }
    plan
}

#[cfg(test)]
mod plan_tests {
    use super::*;
    use crate::dataset::{ItemRef, Reward};

    fn req(kind: RequirementKind, id: &str, qty: i64, qty_max: i64) -> Requirement {
        Requirement { kind, item_id: Some(id.into()), class_name: None, name: id.into(), qty, qty_max, unit: Unit::Count, candidates: vec![], options: vec![] }
    }

    fn reward(result: u32, option: u32, id: &str, qty: i64, kind: RewardKind) -> Reward {
        Reward { result, option, weight: 1.0, owner_only: true, item_id: Some(id.into()), class_name: None, name: id.into(), qty, kind, on_accept: false }
    }

    #[test]
    fn splits_certain_and_uncertain() {
        let any_helmet = Requirement {
            kind: RequirementKind::Entities,
            name: "any helmet".into(),
            qty: 1,
            qty_max: 1,
            candidates: vec![ItemRef { item_id: Some("h1".into()), name: "h1".into(), ..Default::default() }, ItemRef { item_id: Some("h2".into()), name: "h2".into(), ..Default::default() }],
            ..Default::default()
        };
        let c = Contract {
            requirements: vec![
                req(RequirementKind::Entity, "favor", 15, 15),
                req(RequirementKind::Resource, "quant", 24, 24),
                req(RequirementKind::Entity, "range", 2, 5),
                any_helmet,
            ],
            rewards: vec![
                reward(0, 0, "ship", 1, RewardKind::Vehicle),
                reward(1, 0, "scrip", 5, RewardKind::Currency),
                reward(2, 0, "pool-a", 1, RewardKind::Other),
                reward(2, 1, "pool-b", 1, RewardKind::Other),
            ],
            ..Default::default()
        };
        let plan = plan_completion(&c);
        assert_eq!(plan.certain.iter().map(|x| (x.item_id.as_str(), x.delta)).collect::<Vec<_>>(), vec![("favor", -15), ("quant", -24), ("scrip", 5)]);
        assert_eq!(plan.uncertain.iter().map(|x| (x.item_id.as_str(), x.delta)).collect::<Vec<_>>(), vec![("range", -2), ("h1", -1), ("h2", -1), ("pool-a", 1), ("pool-b", 1)]);
    }

    #[test]
    fn mission_rewards_depend_on_ownership() {
        let r = MissionReward { contract_id: "c".into(), debug_name: "Patrol".into(), generator: "X".into(), item_id: "scrip".into(), class_name: "s".into(), name: "MG Scrip".into(), qty: 14, owner_only: true, one_of: false };
        let sample = MissionReward { one_of: true, owner_only: false, item_id: "rgl2".into(), name: "RCMBNT-RGL-2".into(), qty: 1, ..r.clone() };
        assert!(plan_mission_rewards(&[&sample], true).is_empty(), "a one-of set is asked, not applied");
        let owned = plan_mission_rewards(&[&r], true);
        assert_eq!(owned.certain.len(), 1);
        assert_eq!(owned.certain[0].delta, 14);
        let shared = plan_mission_rewards(&[&r], false);
        assert!(shared.certain.is_empty());
        assert_eq!(shared.uncertain.len(), 1);
    }
}
