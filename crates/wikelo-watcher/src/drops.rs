//! What a mission may drop beyond its rewards. Today: the ace pilot. The security givers'
//! patrols, escorts, ambushes and defences roll an extra enemy, the ace pilot, with a chance the
//! contract sets (`AllEncounters_AcePilotChance_BP` or `AcePilotChance_BP`: 0 to 33 % by
//! difficulty in 4.10). He wears the Ace Interceptor helmet (his loadout XML) and his loot
//! table rolls scrip plus, half the time, one item tagged as an ace pilot medal (the pristine
//! medals; the artifact fragments carry no such tag). A chance is not a reward: the watcher never
//! credits any of this to the inventory, the site only lists these missions on the item.

use std::collections::HashSet;

use serde_json::Value;
use starbreaker_cryxml::{CryXml, CryXmlNode};
use starbreaker_p4k::MappedP4k;
use wikelo_core::{Contract, DropHow, DropVia, Item, MissionDrop};

use crate::normalize::{arr, f, s, ty};
use crate::resolver::{RefResolver, record_value, ref_id};

const ACE_NPC: &str = "EntityClassDefinition.PU_Human_Enemy_GroundCombat_NPC_GenericCriminal_AcePilot";

/// Every (mission, item) pair where the ace pilot may show up carrying an item the tracker knows.
/// Missions with a zero chance are left out; so is currency, which these missions already pay as
/// a reward (three scrip off a body are not worth a line).
pub fn ace_pilot_drops(res: &mut RefResolver, p4k: &MappedP4k, items: &[Item], wikelo: &[Contract], warnings: &mut Vec<String>) -> Vec<MissionDrop> {
    let Some(record) = res.find_by_name("EntityClassDefinition", ACE_NPC) else {
        warnings.push(format!("{ACE_NPC}: registro não encontrado; sem loot do ace pilot"));
        return Vec::new();
    };
    let npc = match res.materialize(&record) {
        Ok(v) => record_value(&v),
        Err(e) => {
            warnings.push(format!("{ACE_NPC}: {e}"));
            return Vec::new();
        }
    };
    let worn = worn_classes(res, p4k, &npc, warnings);
    let tags = loot_tags(res, &npc);
    let mut carried: Vec<(&Item, DropHow)> = Vec::new();
    for item in items {
        if item.kind == "currency" {
            continue;
        }
        if worn.contains(&item.class_name.to_ascii_lowercase()) {
            carried.push((item, DropHow::Worn));
        } else if !tags.is_empty() && item_has_tag(res, &item.class_name, &tags) {
            carried.push((item, DropHow::Loot));
        }
    }
    if carried.is_empty() {
        return Vec::new();
    }
    let wikelo_ids: HashSet<&str> = wikelo.iter().map(|c| c.id.as_str()).collect();
    let mut out = Vec::new();
    for (name, record) in res.records_of_type("ContractGenerator") {
        let root = match res.materialize(&record) {
            Ok(v) => record_value(&v),
            Err(e) => {
                warnings.push(format!("{name}: {e}"));
                continue;
            }
        };
        let generator = name.trim_start_matches("ContractGenerator.").to_string();
        for g in arr(&root, "generators") {
            for key in ["contracts", "introContracts", "legacyContracts"] {
                for c in arr(g, key) {
                    let Some(id) = s(c, "id") else { continue };
                    if wikelo_ids.contains(id) {
                        continue;
                    }
                    let Some(chance) = ace_chance(c).filter(|c| *c > 0.0) else { continue };
                    let debug_name = s(c, "debugName").unwrap_or("?").trim().to_string();
                    for (item, how) in &carried {
                        out.push(MissionDrop {
                            contract_id: id.to_string(),
                            debug_name: debug_name.clone(),
                            generator: generator.clone(),
                            item_id: item.id.clone(),
                            class_name: item.class_name.clone(),
                            name: item.name.clone(),
                            chance,
                            how: *how,
                            via: DropVia::AcePilot,
                        });
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.generator.cmp(&b.generator).then(a.debug_name.cmp(&b.debug_name)).then(a.name.cmp(&b.name)));
    out
}

/// The ace chance of one contract: its `…AcePilotChance_BP` override, as the weighted mean of
/// the options (every one seen so far has a single option).
fn ace_chance(c: &Value) -> Option<f64> {
    let props = c.get("paramOverrides").map(|p| arr(p, "propertyOverrides")).unwrap_or(&[]);
    let p = props.iter().find(|p| s(p, "missionVariableName").is_some_and(|n| n.to_ascii_lowercase().ends_with("acepilotchance_bp")))?;
    let opts = p.get("value").map(|v| arr(v, "options")).unwrap_or(&[]);
    let (mut sum, mut weight) = (0.0, 0.0);
    for o in opts {
        let w = f(o, "weighting").unwrap_or(1.0).max(0.0);
        sum += f(o, "value").unwrap_or(0.0) * w;
        weight += w;
    }
    (weight > 0.0).then(|| (sum / weight).clamp(0.0, 1.0))
}

/// The armor the ace wears (`Armor_*` ports of the loadout XML his default-loadout component
/// names), as lowercase entity class names.
fn worn_classes(res: &mut RefResolver, p4k: &MappedP4k, npc: &Value, warnings: &mut Vec<String>) -> HashSet<String> {
    let mut paths = Vec::new();
    for comp in arr(npc, "Components") {
        if ty(comp) != Some("SEntityComponentDefaultLoadoutParams") {
            continue;
        }
        let Some(loadout) = comp.get("loadout") else { continue };
        for a in arr(loadout, "LoadoutAssortments") {
            let Some(assortment) = res.resolve_value(a) else { continue };
            for g in arr(&assortment, "GroupedLoadouts") {
                for l in arr(g, "Loadouts") {
                    if let Some(p) = s(l, "loadoutPath") {
                        paths.push(p.to_string());
                    }
                }
            }
        }
    }
    let mut out = HashSet::new();
    for path in paths {
        let entry = format!("Data\\{}", path.replace('/', "\\"));
        let bytes = match p4k.read_file(&entry) {
            Ok(b) => b,
            Err(e) => {
                warnings.push(format!("{entry}: {e}"));
                continue;
            }
        };
        match starbreaker_cryxml::from_bytes(&bytes) {
            Ok(xml) => collect_armor(&xml, xml.root(), &mut out),
            Err(e) => warnings.push(format!("{entry}: {e:?}")),
        }
    }
    out
}

fn collect_armor(xml: &CryXml, node: &CryXmlNode, out: &mut HashSet<String>) {
    if xml.node_tag(node) == "Item" {
        let (mut port, mut class) = (None, None);
        for (k, v) in xml.node_attributes(node) {
            match k {
                "portName" => port = Some(v),
                "itemName" => class = Some(v),
                _ => {}
            }
        }
        if let (Some(p), Some(c)) = (port, class)
            && p.starts_with("Armor_")
            && !c.is_empty()
        {
            out.insert(c.to_ascii_lowercase());
        }
    }
    for child in xml.node_children(node) {
        collect_armor(xml, child, out);
    }
}

/// The tags the ace's loot archetypes draw from (`LootTable` → `LootArchetype` → entry tags).
fn loot_tags(res: &mut RefResolver, npc: &Value) -> HashSet<String> {
    let mut tags = HashSet::new();
    for comp in arr(npc, "Components") {
        if ty(comp) != Some("LootGenerationComponentParams") {
            continue;
        }
        let Some(table) = comp.get("lootConfig").and_then(|c| c.get("lootTable")).and_then(|t| res.resolve_value(t)) else { continue };
        for weighted in arr(&table, "lootArchetypes") {
            let Some(archetype) = weighted.get("archetype").and_then(|a| res.resolve_value(a)) else { continue };
            let mut groups: Vec<&Value> = archetype.get("primaryOrGroup").into_iter().collect();
            groups.extend(arr(&archetype, "secondaryOrGroups"));
            for g in groups {
                for e in arr(g, "entries") {
                    if let Some(id) = e.get("tag").and_then(ref_id) {
                        tags.insert(id.to_string());
                    }
                }
            }
        }
    }
    tags
}

/// Does the item's entity class carry one of the tags?
fn item_has_tag(res: &mut RefResolver, class_name: &str, tags: &HashSet<String>) -> bool {
    let Some(record) = res.find_by_name("EntityClassDefinition", &format!("EntityClassDefinition.{class_name}")) else { return false };
    let Ok(v) = res.materialize(&record) else { return false };
    let v = record_value(&v);
    arr(&v, "tags").iter().filter_map(ref_id).any(|id| tags.contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chance_is_the_override_weighted_mean() {
        let c = json!({ "paramOverrides": { "propertyOverrides": [
            { "missionVariableName": "Other_BP", "value": { "options": [{ "value": 0.9, "weighting": 1.0 }] } },
            { "missionVariableName": "AllEncounters_AcePilotChance_BP", "value": { "options": [{ "value": 0.2, "weighting": 1.0 }, { "value": 0.4, "weighting": 3.0 }] } }
        ] } });
        assert!((ace_chance(&c).unwrap() - 0.35).abs() < 1e-9);
        let short = json!({ "paramOverrides": { "propertyOverrides": [{ "missionVariableName": "AcePilotChance_BP", "value": { "options": [{ "value": 0.0, "weighting": 1.0 }] } }] } });
        assert_eq!(ace_chance(&short), Some(0.0), "a zero chance is a value, the caller drops it");
        assert_eq!(ace_chance(&json!({ "paramOverrides": { "propertyOverrides": [] } })), None);
        assert_eq!(ace_chance(&json!({})), None);
    }
}
