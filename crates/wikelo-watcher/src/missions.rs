//! The face of the other givers' missions the tracker mentions: those that pay a known item
//! (`mission_rewards`) and those the ace pilot may show up in (`mission_drops`) only carry
//! contract ids; this collects, per contract, what a player needs to recognise it and to know
//! whether they can take it: the localized title and description, who gives it (the faction of
//! its reputation prerequisite), the system it is in, its family (patrol, ambush…), the
//! reputation it asks for and pays, the missions that must be completed before it (the game
//! chains them through completion tags), and whether the build offers it at all (not-for-release
//! or work-in-progress on the generator or the contract, like `Contract::is_hidden` for Wikelo's
//! own). Missions a chain leads to are described too, so the site can name them.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde_json::Value;
use wikelo_core::{MissionDrop, MissionInfo, MissionReward, RepRequirement};

use crate::loc::{Localization, plain_text};
use crate::normalize::{arr, b, i, s, ty};
use crate::resolver::{RefResolver, record_value, ref_id};

const SYSTEMS: [&str; 3] = ["Stanton", "Pyro", "Nyx"];

/// What pass one keeps of every contract: the tags it needs completed first, which is how the
/// game chains missions (the tags themselves are granted by other contracts' completion results).
struct Link {
    required: Vec<String>,
}

/// One entry per contract named by a reward or a drop, plus the ones their chains lead to,
/// sorted by id.
pub fn load_missions(res: &mut RefResolver, loc: &Localization, rewards: &[MissionReward], drops: &[MissionDrop], warnings: &mut Vec<String>) -> Vec<MissionInfo> {
    let wanted: HashSet<&str> = rewards.iter().map(|r| r.contract_id.as_str()).chain(drops.iter().map(|d| d.contract_id.as_str())).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let mut roots: Vec<(String, Rc<Value>)> = Vec::new();
    for (name, record) in res.records_of_type("ContractGenerator") {
        match res.materialize(&record) {
            Ok(v) => roots.push((name.trim_start_matches("ContractGenerator.").to_string(), record_value(&v))),
            Err(e) => warnings.push(format!("{name}: {e}")),
        }
    }

    // Pass one: the links of every contract of every generator, so a chain can be followed past
    // the missions the tracker names.
    let mut links: HashMap<String, Link> = HashMap::new();
    let mut grants_by_tag: HashMap<String, Vec<String>> = HashMap::new();
    for (_, root) in &roots {
        for g in arr(root, "generators") {
            let generator_prereqs = g.get("defaultAvailability").map(|d| arr(d, "prerequisites")).unwrap_or(&[]);
            for c in contracts_of(g) {
                let Some(id) = s(c, "id") else { continue };
                let mut required = Vec::new();
                for p in generator_prereqs.iter().chain(arr(c, "additionalPrerequisites")) {
                    if ty(p) == Some("ContractPrerequisite_CompletedContractTags") {
                        required.extend(p.get("requiredCompletedContractTags").map(|l| arr(l, "tags")).unwrap_or(&[]).iter().filter_map(ref_id).map(str::to_string));
                    }
                }
                for r in results_of(c) {
                    if ty(r) == Some("ContractResult_CompletionTags") {
                        for tag in arr(r, "completionTags").iter().filter_map(|ct| ct.get("tag").and_then(ref_id)) {
                            grants_by_tag.entry(tag.to_string()).or_default().push(id.to_string());
                        }
                    }
                }
                links.insert(id.to_string(), Link { required });
            }
        }
    }
    let unlockers = |id: &str| -> Vec<String> {
        let mut ids: Vec<String> = links
            .get(id)
            .map(|l| l.required.iter().flat_map(|t| grants_by_tag.get(t).into_iter().flatten()).filter(|other| other.as_str() != id).cloned().collect())
            .unwrap_or_default();
        ids.sort();
        ids.dedup();
        ids
    };
    // The missions to describe: the ones named, plus whatever must be completed before them, as
    // deep as the chains go.
    let mut closure: HashSet<String> = wanted.iter().map(|id| id.to_string()).collect();
    let mut queue: Vec<String> = closure.iter().cloned().collect();
    while let Some(id) = queue.pop() {
        for other in unlockers(&id) {
            if closure.insert(other.clone()) {
                queue.push(other);
            }
        }
    }

    // Pass two: the face of each mission in the closure.
    let mut givers: HashMap<String, Option<String>> = HashMap::new();
    let mut out = Vec::new();
    for (generator, root) in &roots {
        for g in arr(root, "generators") {
            let generator_off = flag(g, "notForRelease") || flag(g, "workInProgress");
            let generator_prereqs = g.get("defaultAvailability").map(|d| arr(d, "prerequisites")).unwrap_or(&[]);
            for c in contracts_of(g) {
                let Some(id) = s(c, "id") else { continue };
                if !closure.contains(id) {
                    continue;
                }
                let debug_name = s(c, "debugName").unwrap_or("?").trim().to_string();
                let (mut title_key, mut description_key) = (None, None);
                for o in c.get("paramOverrides").map(|p| arr(p, "stringParamOverrides")).unwrap_or(&[]) {
                    match (s(o, "param"), s(o, "value")) {
                        (Some("Title"), Some(v)) => title_key = Some(v),
                        (Some("Description"), Some(v)) => description_key = Some(v),
                        _ => {}
                    }
                }
                // A contract without its own title and description shows the template's (Project
                // Hyperion's phases: `contractDisplayInfo.displayString` = title, short title,
                // description, contractor). The template also names the mission type.
                let template_ref = c.get("template").cloned();
                let template = template_ref.as_ref().and_then(|t| res.resolve_value(t));
                let display: Vec<&str> = template.as_ref().map(|t| t.get("contractDisplayInfo").map(|d| arr(d, "displayString")).unwrap_or(&[]).iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                let title_key = title_key.or_else(|| display.first().copied());
                let description_key = description_key.or_else(|| display.get(2).copied());
                let title = title_key.and_then(|k| loc.text_opt(k)).map(|t| plain_text(&t)).unwrap_or_else(|| debug_name.clone());
                let description_plain = description_key.and_then(|k| loc.text_opt(k)).map(|d| plain_text(&d)).unwrap_or_default();
                let mission_type = c
                    .get("paramOverrides")
                    .and_then(|p| p.get("missionTypeOverride"))
                    .filter(|v| !v.is_null())
                    .or_else(|| g.get("contractParams").and_then(|p| p.get("missionTypeOverride")).filter(|v| !v.is_null()))
                    .cloned()
                    .or_else(|| template.as_ref().and_then(|t| t.get("contractDisplayInfo")).and_then(|d| d.get("type")).cloned());
                let event = mission_type.as_ref().is_some_and(|v| is_event_type(res, v));
                let (mut giver, mut system, mut rep_required) = (None, None, None);
                for p in generator_prereqs.iter().chain(arr(c, "additionalPrerequisites")) {
                    match ty(p) {
                        Some("ContractPrerequisite_Reputation") => {
                            if let Some(faction) = p.get("factionReputation") {
                                let key = ref_id(faction).unwrap_or("").to_string();
                                let name = givers.entry(key).or_insert_with(|| res.resolve_value(faction).and_then(|v| s(&v, "displayName").map(|k| loc.text(k)))).clone();
                                if name.is_some() {
                                    giver = name;
                                }
                            }
                            let min = p.get("minStanding").and_then(|v| standing(res, loc, v));
                            let max = p.get("maxStanding").and_then(|v| standing(res, loc, v));
                            rep_required = Some(RepRequirement {
                                scope_id: p.get("scope").and_then(ref_id).map(str::to_string),
                                min_rep: min.as_ref().map(|st| st.1),
                                min_standing_name: min.map(|st| st.0),
                                max_standing_name: max.map(|st| st.0),
                            });
                        }
                        Some("ContractPrerequisite_Location") if system.is_none() => {
                            system = p.get("locationAvailable").and_then(|l| s(l, "$path")).and_then(system_of_path);
                        }
                        Some("ContractPrerequisite_Locality") if system.is_none() => {
                            system = p.get("localityAvailable").and_then(path_of).and_then(system_of_locality);
                        }
                        _ => {}
                    }
                }
                let template_name = template_ref.as_ref().and_then(|t| s(t, "$name").or_else(|| s(t, "$path")).or_else(|| t.as_str())).unwrap_or("");
                out.push(MissionInfo {
                    id: id.to_string(),
                    debug_name: debug_name.clone(),
                    generator: generator.clone(),
                    title,
                    description_plain,
                    giver,
                    system: system_of_name(&debug_name).or(system),
                    family: family_of(template_name, generator, &debug_name).map(str::to_string),
                    rep_required,
                    rep_reward: rep_reward(res, c),
                    requires: unlockers(id),
                    event,
                    disabled: generator_off || flag(c, "notForRelease") || flag(c, "workInProgress"),
                });
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// The contracts of one generator entry, in every list the game keeps them in.
fn contracts_of(g: &Value) -> impl Iterator<Item = &Value> {
    ["contracts", "introContracts", "legacyContracts"].into_iter().flat_map(move |key| arr(g, key).iter())
}

fn results_of(c: &Value) -> &[Value] {
    c.get("contractResults").map(|r| arr(r, "contractResults")).unwrap_or(&[])
}

fn flag(v: &Value, key: &str) -> bool {
    b(v, key).unwrap_or(false)
}

/// A reference as the resolver emits it (`$path` object) or as a plain path string.
fn path_of(v: &Value) -> Option<&str> {
    s(v, "$path").or_else(|| v.as_str())
}

/// Is this mission type an event one? The priority types (`MissionType.Priority*`, `IASI` for
/// the RSI Discovery Month) share the priority icon; the record name and the icon both tell.
fn is_event_type(res: &mut RefResolver, v: &Value) -> bool {
    let named = |text: &str| {
        let t = text.to_ascii_lowercase();
        t.contains("priority") || t.contains("iasi")
    };
    if res.ref_name(v).is_some_and(|n| named(&n)) || path_of(v).is_some_and(named) {
        return true;
    }
    res.resolve_value(v).is_some_and(|t| s(&t, "svgIconPath").is_some_and(named) || s(&t, "LocalisedTypeName").is_some_and(named))
}

/// `missionResults[0]`: the result fires on success. Mirrors `normalize::applies_on_success`.
fn on_success(r: &Value) -> bool {
    arr(r, "missionResults").first().and_then(Value::as_bool).unwrap_or(true)
}

/// Reputation paid on success (`SReputationRewardAmount.reputationAmount`), when the mission pays any.
fn rep_reward(res: &mut RefResolver, c: &Value) -> Option<i64> {
    results_of(c).iter().filter(|r| ty(r) == Some("ContractResult_LegacyReputation") && on_success(r)).find_map(|r| {
        r.get("contractResultReputationAmounts").and_then(|a| a.get("reward")).and_then(|v| res.resolve_value(v)).and_then(|v| i(&v, "reputationAmount"))
    })
}

/// `SReputationStandingParams` → (display name, minimum reputation). Mirrors `Normalizer::standing`.
fn standing(res: &mut RefResolver, loc: &Localization, sref: &Value) -> Option<(String, i64)> {
    let v = res.resolve_value(sref)?;
    Some((s(&v, "displayName").map(|k| loc.text(k)).unwrap_or_else(|| "?".into()), i(&v, "minReputation").unwrap_or(0)))
}

/// The kind of mission, from the names: what the site groups the lines under. The generator and
/// the debug name say what the giver calls it (an escort runs on a "defend" template), so they
/// are read before the template; defend-and-escort missions are defences.
fn family_of(template: &str, generator: &str, debug_name: &str) -> Option<&'static str> {
    let kind = |hay: &str| {
        let has = |needles: &[&str]| needles.iter().any(|n| hay.contains(n));
        if has(&["ambush"]) {
            Some("ambush")
        } else if has(&["patrol"]) {
            Some("patrol")
        } else if has(&["defend"]) {
            Some("defend")
        } else if has(&["escort"]) {
            Some("escort")
        } else if has(&["delve", "investigat", "recoverdata", "recoverencrypted"]) {
            Some("investigation")
        } else if has(&["pilotschool", "trainingexercise"]) {
            Some("training")
        } else {
            None
        }
    };
    kind(&format!("{generator} {debug_name}").to_ascii_lowercase()).or_else(|| kind(&template.to_ascii_lowercase()))
}

/// The system named in the debug name (`HeadHunters_Patrol_Nyx_Easy`, `…-Stanton4_Repeat`), when
/// there is one: the Nyx missions carry no location prerequisite, the name is all there is.
fn system_of_name(debug_name: &str) -> Option<String> {
    debug_name
        .split(['_', '-'])
        .map(|t| t.trim_end_matches(|c: char| c.is_ascii_digit()))
        .find_map(|t| SYSTEMS.iter().find(|sys| sys.eq_ignore_ascii_case(t)).map(|sys| sys.to_string()))
}

/// `…/records/starmap/pu/system/stanton/stanton1/…` → `Stanton`: the star map lays its records
/// out by system, and a location prerequisite points at one of them.
fn system_of_path(path: &str) -> Option<String> {
    let segment = path.split("/system/").nth(1)?.split('/').next()?;
    let mut chars = segment.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().collect::<String>() + chars.as_str())
}

/// `…/pu_missionlocality/stanton1.json` → `Stanton`; a locality that is not a system
/// (`regiona`) says nothing.
fn system_of_locality(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?.strip_suffix(".json").unwrap_or("");
    let name = file.trim_end_matches(|c: char| c.is_ascii_digit());
    SYSTEMS.iter().find(|sys| sys.eq_ignore_ascii_case(name)).map(|sys| sys.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_comes_from_the_name_or_the_star_map_path() {
        assert_eq!(system_of_name("HeadHunters_Patrol_Nyx_Easy_V").as_deref(), Some("Nyx"));
        assert_eq!(system_of_name("FoxwellEnforcement_ShipAmbush_Pyro_Easy").as_deref(), Some("Pyro"));
        assert_eq!(system_of_name("Hockrow_FacilityDelve_P2M1-Stanton4_Repeat").as_deref(), Some("Stanton"), "a planet token names its system");
        assert_eq!(system_of_name("GillysPilotSchool_Mission01"), None);
        assert_eq!(system_of_path("file://./../../libs/foundry/records/starmap/pu/system/stanton/stanton1/starmapobject.stanton1.json").as_deref(), Some("Stanton"));
        assert_eq!(system_of_path("file://./../../libs/foundry/records/starmap/pu/system/pyro/starmapobject.pyrostar.json").as_deref(), Some("Pyro"));
        assert_eq!(system_of_path("file://./x/y.json"), None);
        assert_eq!(system_of_locality("file://./../../libs/foundry/records/missiondata/pu_missionlocality/nyx.json").as_deref(), Some("Nyx"));
        assert_eq!(system_of_locality("file://./../../libs/foundry/records/missiondata/pu_missionlocality/stanton1.json").as_deref(), Some("Stanton"));
        assert_eq!(system_of_locality("file://./../../libs/foundry/records/missiondata/pu_missionlocality/regiona.json"), None);
    }

    #[test]
    fn the_family_comes_from_the_names() {
        assert_eq!(family_of("ContractTemplate.SecurityPatrol", "HeadHunters_Patrol", "HeadHunters_Patrol_Nyx_Easy"), Some("patrol"));
        assert_eq!(family_of("ContractTemplate.ShipAmbush", "FoxwellEnforcement_Ambush", "FoxwellEnforcement_ShipAmbush_Pyro_Easy"), Some("ambush"));
        assert_eq!(family_of("…/defendentitesandescort_2phases.json", "Foxwell_DefendEntitiesAndEscort", "x"), Some("defend"));
        assert_eq!(family_of("…/defendentitesandescort_2phases.json", "HeadHunters_EscortShips", "HeadHunters_EscortShips_Nyx_Hard"), Some("escort"), "the giver calls it an escort");
        assert_eq!(family_of("", "HockrowAgency_FacilityDelve", "Hockrow_FacilityDelve_P1M3"), Some("investigation"));
        assert_eq!(family_of("", "Unaffiliated_Generator", "GillysPilotSchool_Mission01"), Some("training"));
        assert_eq!(family_of("", "Battaglia_Generator", "Battaglia_BoardShip_Intro"), None);
    }
}
