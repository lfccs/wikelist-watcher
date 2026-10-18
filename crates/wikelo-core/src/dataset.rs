//! Normalized Wikelo dataset (`wikelo-data.json`), schema version 1.
//!
//! Everything here is produced from the player's own `Data.p4k` by
//! `wikelo-watcher`. Field names are camelCase on the wire so the React
//! frontend can consume the JSON directly.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// Wikelo Emporium faction GUID in DataForge.
pub const WIKELO_FACTION_ID: &str = "5f442fdd-15cc-4f62-bd10-fce2023f4d0c";
/// Name of the contract generator record that owns every Wikelo contract.
pub const GENERATOR_RECORD_NAME: &str = "ContractGenerator.TheCollector";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WikeloDataset {
    pub schema_version: u32,
    /// Always "local-extract" for now; kept so future sources can be told apart.
    pub source: String,
    pub build: BuildInfo,
    /// RFC 3339 timestamp of extraction.
    pub generated_at: String,
    pub faction: Faction,
    pub emporiums: Vec<Emporium>,
    pub contracts: Vec<Contract>,
    pub items: Vec<Item>,
    pub favor_conversions: Vec<FavorConversion>,
    /// Crafting recipes for items that contracts ask for or hand out.
    #[serde(default)]
    pub crafting: Vec<CraftRecipe>,
    /// Extraction warnings (unknown struct types, unresolved references...).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Rewards paid by missions of *other* contract givers (patrols, bounties, data
    /// recovery...) in items this tracker knows, e.g. MG Scrip. Lets the log watcher
    /// credit the inventory when such a mission completes.
    #[serde(default)]
    pub mission_rewards: Vec<MissionReward>,
    /// Which extractor build produced this dataset and from which game files, so two
    /// extractions can be compared (see `validate::canonical_hash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor: Option<ExtractorInfo>,
}

/// Provenance of a dataset: the extractor and the exact game files it read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExtractorInfo {
    /// `wikelo-watcher`.
    pub name: String,
    /// Cargo package version.
    pub version: String,
    /// Git commit the binary was built from (`dev` when unknown).
    pub commit: String,
    /// SHA-256 of `Data\Game2.dcb` as read from the p4k.
    pub dcb_sha256: String,
    /// SHA-256 of `Data\Localization\english\global.ini`.
    pub ini_sha256: String,
}

/// One item reward of a non-Wikelo mission, with the quantity from the game data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MissionReward {
    /// Contract definition GUID (`contractDefinitionId` in Game.log).
    pub contract_id: String,
    pub debug_name: String,
    /// Generator record name without the `ContractGenerator.` prefix.
    pub generator: String,
    pub item_id: String,
    pub class_name: String,
    pub name: String,
    pub qty: i64,
    /// Only the player who owns the mission receives it (party members do not).
    pub owner_only: bool,
    /// The mission pays exactly one of the items flagged like this for the same contract, and
    /// which one is the player's choice inside the mission, which Game.log does not record
    /// (Project Hyperion's recombinant samples). `qty` is per item. The site asks the player.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub one_of: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BuildInfo {
    /// Channel folder name, uppercase: LIVE, PTU, EPTU, HOTFIX, TECH-PREVIEW.
    pub channel: String,
    pub branch: String,
    pub version: String,
    pub p4_change: String,
    pub build_id: String,
    pub tag: String,
    pub dcb_version: Option<u32>,
    pub p4k_size: Option<u64>,
}

impl BuildInfo {
    /// Stable directory key for this build: `LIVE-4.10.197.2878-12913470`.
    pub fn cache_key(&self) -> String {
        fn sanitize(s: &str) -> String {
            let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
            if t.is_empty() { "unknown".into() } else { t }
        }
        format!("{}-{}-{}", self.channel.to_ascii_uppercase(), sanitize(&self.version), sanitize(&self.p4_change))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Faction {
    pub id: String,
    pub name: String,
    pub scopes: Vec<RepScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RepScope {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub ceiling: Option<i64>,
    pub standings: Vec<Standing>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Standing {
    pub id: Option<String>,
    pub name: String,
    pub display_name: String,
    pub min_rep: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Emporium {
    /// Stable short key: dasi, selo, kinga (or the class name when unknown).
    pub key: String,
    pub name: String,
    pub location_hash: Option<u64>,
    /// Every spelling the game uses for this location in data and logs.
    pub class_names: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Vehicle,
    Weapon,
    Armor,
    Favour,
    Trade,
    Intro,
    #[default]
    Other,
}

impl Category {
    /// Display order used when sorting contracts.
    pub fn order(self) -> u8 {
        match self {
            Category::Vehicle => 0,
            Category::Weapon => 1,
            Category::Armor => 2,
            Category::Favour => 3,
            Category::Trade => 4,
            Category::Intro => 5,
            Category::Other => 6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Contract {
    /// Contract definition GUID. Same value as `contractDefinitionId[...]` in Game.log.
    pub id: String,
    pub debug_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub category: Category,
    pub mission_type: Option<String>,
    pub title_key: Option<String>,
    pub title: String,
    pub description_key: Option<String>,
    pub description: String,
    /// Description with `<EM4>` markup and `~mission(...)` tokens stripped.
    pub description_plain: String,
    pub rep_required: Option<RepRequirement>,
    #[serde(default)]
    pub rep_gained: Vec<RepGain>,
    pub once_only: bool,
    pub not_for_release: bool,
    pub work_in_progress: bool,
    pub hidden_in_mobiglas: bool,
    pub cooldown: Cooldown,
    pub lifetime: Lifetime,
    pub prerequisites: Prerequisites,
    #[serde(default)]
    pub completion_tags: Vec<TagRef>,
    #[serde(default)]
    pub emporium_keys: Vec<String>,
    /// Filled from Game.log (`GenerateLocationProperty ... contract: X`), None when unknown.
    pub offered_in_build: Option<bool>,
    pub requirements: Vec<Requirement>,
    pub rewards: Vec<Reward>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RepRequirement {
    pub scope_id: Option<String>,
    pub min_rep: Option<i64>,
    pub min_standing_name: Option<String>,
    pub max_standing_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RepGain {
    pub scope_id: Option<String>,
    pub scope_name: Option<String>,
    pub amount: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Cooldown {
    pub abandoned_sec: i64,
    pub personal_sec: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Lifetime {
    /// Raw `instanceLifeTime` from the game data (unit as stored by CIG).
    pub instance_lifetime: Option<f64>,
    /// Raw `respawnTime` from the game data.
    pub respawn_time: Option<f64>,
    pub max_instances: Option<i64>,
    pub max_per_player: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Prerequisites {
    #[serde(default)]
    pub required_tags: Vec<TagRef>,
    #[serde(default)]
    pub excluded_tags: Vec<TagRef>,
    pub required_count: Option<i64>,
    /// Contract ids whose completion tag is required (resolved from tags when possible).
    #[serde(default)]
    pub required_contract_ids: Vec<String>,
    pub crime_stat: Option<CrimeStatRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CrimeStatRange {
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TagRef {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum RequirementKind {
    /// One specific entity class, counted in units.
    Entity,
    /// Any of several entity classes (candidates), counted in units.
    Entities,
    /// A commodity measured in SCU.
    Resource,
    /// A mission item resolved through a tag search.
    MissionItem,
    /// One of several alternative requirement sets (`options`).
    Or,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    #[default]
    Count,
    Scu,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    pub kind: RequirementKind,
    pub item_id: Option<String>,
    pub class_name: Option<String>,
    pub name: String,
    pub qty: i64,
    pub qty_max: i64,
    pub unit: Unit,
    #[serde(default)]
    pub candidates: Vec<ItemRef>,
    /// For `Or` requirements: each option is a list that must be fully satisfied.
    #[serde(default)]
    pub options: Vec<Vec<Requirement>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ItemRef {
    pub item_id: Option<String>,
    pub class_name: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum RewardKind {
    Vehicle,
    Armor,
    Weapon,
    Currency,
    Favour,
    Blueprint,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Reward {
    /// Index of the `contractResults` block this reward came from.
    pub result: u32,
    /// Weighted alternative within that block. Rewards sharing `result` and
    /// `option` are awarded together; different `option`s are alternatives.
    pub option: u32,
    pub weight: f64,
    pub owner_only: bool,
    pub item_id: Option<String>,
    pub class_name: Option<String>,
    pub name: String,
    pub qty: i64,
    pub kind: RewardKind,
    /// Granted when the contract is accepted (e.g. the blueprint needed to craft
    /// the item the contract asks for), not on completion.
    #[serde(default)]
    pub on_accept: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub class_name: String,
    pub name: String,
    pub item_type: Option<String>,
    pub sub_type: Option<String>,
    /// Loose classification for the UI: currency, favour, commodity, part, armor, weapon, vehicle, other.
    pub kind: String,
    /// Image extracted from the game files, relative to the dataset directory (`img/<name>.png|svg`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Which static texture set the image came from (`vehicle_icon`, `frontend_ship`, `loadout_icon`, `inv_icon`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FavorConversion {
    pub contract_id: String,
    pub give: Vec<QtyRef>,
    pub receive: Vec<QtyRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QtyRef {
    pub item_id: Option<String>,
    pub name: String,
    pub qty: i64,
    pub unit: Unit,
}

/// One ingredient option inside a crafting slot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CraftInput {
    /// `resource` (SCU of a mineable/refined resource) or `item` (an entity class).
    pub kind: String,
    pub item_id: Option<String>,
    pub class_name: Option<String>,
    pub name: String,
    pub qty: f64,
    pub unit: Unit,
    /// Minimum quality the game demands for this ingredient (0 = any).
    pub min_quality: i64,
}

/// A slot of the recipe: pick `count` of `options`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CraftSlot {
    pub name: String,
    pub count: u32,
    pub options: Vec<CraftInput>,
}

/// Where the blueprint comes from, when a Wikelo contract hands it out.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CraftSource {
    pub contract_id: String,
    pub contract_title: String,
    /// `accept` (granted when the contract is accepted) or `complete`.
    pub when: String,
    pub chance: f64,
}

/// A `CraftingBlueprintRecord` boiled down to what a player needs to farm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct CraftRecipe {
    pub blueprint_id: String,
    pub blueprint_name: String,
    pub output_item_id: String,
    pub output_class_name: String,
    pub output_name: String,
    pub category: Option<String>,
    pub craft_time_sec: f64,
    pub slots: Vec<CraftSlot>,
    pub sources: Vec<CraftSource>,
}

/// A mission name without the instance index the game appends to some of them in Game.log
/// (`GillysPilotSchool_Mission06_0` is the contract `GillysPilotSchool_Mission06`). Names that
/// do not end in `_<digits>` come back unchanged.
pub fn instance_base(name: &str) -> &str {
    let name = name.trim();
    match name.rsplit_once('_') {
        Some((base, suffix)) if !base.is_empty() && !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => name,
    }
}

impl WikeloDataset {
    /// Rewards of a non-Wikelo mission, matched by contract GUID first, then by debug name
    /// (case-insensitive, with or without the `_<n>` instance index Game.log may append).
    pub fn mission_rewards_for(&self, contract_id: Option<&str>, debug_name: Option<&str>) -> Vec<&MissionReward> {
        if let Some(id) = contract_id {
            let by_id: Vec<&MissionReward> = self.mission_rewards.iter().filter(|r| r.contract_id == id).collect();
            if !by_id.is_empty() {
                return by_id;
            }
        }
        let Some(dn) = debug_name else { return Vec::new() };
        for wanted in [dn.trim(), instance_base(dn)] {
            let found: Vec<&MissionReward> = self.mission_rewards.iter().filter(|r| r.debug_name.eq_ignore_ascii_case(wanted)).collect();
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }

    /// Recipe whose output is `item_id`, if the game lets you craft it.
    pub fn recipe_for(&self, item_id: &str) -> Option<&CraftRecipe> {
        self.crafting.iter().find(|r| r.output_item_id == item_id)
    }

    pub fn contract(&self, id: &str) -> Option<&Contract> {
        self.contracts.iter().find(|c| c.id == id)
    }

    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    /// Find a contract by its current debug name or any recorded alias.
    pub fn contract_by_debug_name(&self, debug_name: &str) -> Option<&Contract> {
        let wanted = debug_name.trim();
        self.contracts
            .iter()
            .find(|c| c.debug_name.trim() == wanted || c.aliases.iter().any(|a| a.trim() == wanted))
    }
}

impl Category {
    /// Derive the UI category from the localization key, the mission type,
    /// the debug name and, as a last resort, the reward kinds.
    pub fn derive(
        title_key: Option<&str>,
        mission_type: Option<&str>,
        debug_name: &str,
        rewards: &[Reward],
    ) -> Category {
        let reward_kind = |k: RewardKind| rewards.iter().any(|r| r.kind == k);
        if let Some(key) = title_key {
            let key = key.trim_start_matches('@');
            if key.starts_with("TheCollector_Ships_")
                || key.starts_with("TheCollector_ShipMod_")
                || key.starts_with("TheCollector_Mod_")
            {
                return Category::Vehicle;
            }
            if key.starts_with("TheCollector_Conversion_") {
                return Category::Favour;
            }
            if key.starts_with("TheCollector_Trade_") {
                return Category::Trade;
            }
            if key.starts_with("TheCollector_Intro") {
                return Category::Intro;
            }
            if key.starts_with("TheCollector_Recipes_") {
                if reward_kind(RewardKind::Armor) {
                    return Category::Armor;
                }
                if reward_kind(RewardKind::Weapon) {
                    return Category::Weapon;
                }
            }
        }
        if let Some(mt) = mission_type
            && mt.contains("Vehicle")
        {
            return Category::Vehicle;
        }
        let dn = debug_name.to_ascii_lowercase();
        if dn.contains("_vehicle") {
            return Category::Vehicle;
        }
        if dn.contains("_favours") || dn.contains("_favors") {
            return Category::Favour;
        }
        if dn.contains("_intro") {
            return Category::Intro;
        }
        if dn.contains("_gg_") {
            return Category::Trade;
        }
        if reward_kind(RewardKind::Vehicle) {
            return Category::Vehicle;
        }
        if reward_kind(RewardKind::Armor) {
            return Category::Armor;
        }
        if reward_kind(RewardKind::Weapon) {
            return Category::Weapon;
        }
        if reward_kind(RewardKind::Favour) || reward_kind(RewardKind::Currency) {
            return Category::Favour;
        }
        Category::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reward(kind: RewardKind) -> Reward {
        Reward {
            result: 0,
            option: 0,
            weight: 1.0,
            owner_only: true,
            item_id: None,
            class_name: None,
            name: "x".into(),
            qty: 1,
            kind,
            on_accept: false,
        }
    }

    #[test]
    fn category_from_title_key() {
        assert_eq!(
            Category::derive(
                Some("@TheCollector_Ships_Scorpius_TItle"),
                None,
                "TheCollector_Vehicle_Medium_Scorpius",
                &[]
            ),
            Category::Vehicle
        );
        assert_eq!(
            Category::derive(
                Some("TheCollector_Recipes_Title_SandArmour"),
                None,
                "TheCollector_OrbArm",
                &[reward(RewardKind::Armor)]
            ),
            Category::Armor
        );
        assert_eq!(
            Category::derive(
                Some("TheCollector_Recipes_Title_ZipZap"),
                None,
                "TheCollector_AD_ZapZip",
                &[reward(RewardKind::Weapon)]
            ),
            Category::Weapon
        );
        assert_eq!(
            Category::derive(
                Some("TheCollector_Conversion_Favors_Title_1"),
                None,
                "TheCollector_Favours_MercScrip",
                &[]
            ),
            Category::Favour
        );
        assert_eq!(
            Category::derive(Some("TheCollector_Intro_Title"), None, "TheCollector_Intro", &[]),
            Category::Intro
        );
    }

    #[test]
    fn category_falls_back_to_debug_name() {
        assert_eq!(
            Category::derive(None, None, "TheColllector_Vehicle_Ground_ATLS_RedNBlue", &[]),
            Category::Vehicle
        );
        assert_eq!(
            Category::derive(None, None, "TheCollector_Favours_PolarisParts", &[]),
            Category::Favour
        );
        assert_eq!(Category::derive(None, None, "TheCollector_FoodOrder", &[]), Category::Other);
    }

    #[test]
    fn dataset_roundtrips_json() {
        let ds = WikeloDataset {
            schema_version: SCHEMA_VERSION,
            source: "local-extract".into(),
            build: BuildInfo { channel: "LIVE".into(), ..Default::default() },
            generated_at: "2026-10-14T00:00:00Z".into(),
            faction: Faction::default(),
            emporiums: vec![],
            contracts: vec![],
            items: vec![],
            favor_conversions: vec![],
            mission_rewards: vec![],
            crafting: vec![],
            warnings: vec![],
            extractor: None,
        };
        let json = serde_json::to_string(&ds).unwrap();
        assert!(json.contains("\"schemaVersion\":1"));
        let back: WikeloDataset = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ds);
    }
}

#[cfg(test)]
mod mission_reward_tests {
    use super::*;

    fn reward(contract_id: &str, debug_name: &str, qty: i64) -> MissionReward {
        MissionReward {
            contract_id: contract_id.into(),
            debug_name: debug_name.into(),
            generator: "Unaffiliated_Generator".into(),
            item_id: "scrip".into(),
            class_name: "Scrip_Merc_1".into(),
            name: "MG Scrip".into(),
            qty,
            owner_only: true,
            one_of: false,
        }
    }

    #[test]
    fn instance_suffix_is_optional() {
        assert_eq!(instance_base("GillysPilotSchool_Mission06_0"), "GillysPilotSchool_Mission06");
        assert_eq!(instance_base("GillysPilotSchool_Mission06"), "GillysPilotSchool_Mission06");
        assert_eq!(instance_base("TheCollector_Favours_PolarisParts"), "TheCollector_Favours_PolarisParts");
        assert_eq!(instance_base(" HeadHunters_Patrol_12 "), "HeadHunters_Patrol");
        assert_eq!(instance_base("_7"), "_7");
    }

    #[test]
    fn mission_rewards_match_by_id_then_by_name_with_or_without_the_index() {
        let mut ds: WikeloDataset = serde_json::from_value(serde_json::json!({
            "schemaVersion": 1, "source": "test",
            "build": {"channel": "LIVE", "branch": "b", "version": "4.10.1.1", "p4Change": "1", "buildId": "", "tag": "", "dcbVersion": 8, "p4kSize": 1},
            "generatedAt": "2026-01-01T00:00:00Z",
            "faction": {"id": "w", "name": "Wikelo", "scopes": []},
            "emporiums": [], "contracts": [], "items": [], "favorConversions": [], "crafting": [], "warnings": [], "missionRewards": []
        }))
        .expect("fixture");
        ds.mission_rewards = vec![reward("id-6", "GillysPilotSchool_Mission06", 14), reward("id-5", "GillysPilotSchool_Mission05", 12), reward("id-5", "GillysPilotSchool_Mission05", 1)];

        let by_id = ds.mission_rewards_for(Some("id-5"), None);
        assert_eq!(by_id.iter().map(|r| r.qty).collect::<Vec<_>>(), vec![12, 1]);
        // The log's contractDefinitionId is not always the generator entry id: fall back to the name.
        let logged = ds.mission_rewards_for(Some("ce36fccc-other"), Some("GillysPilotSchool_Mission06_0"));
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].qty, 14);
        assert_eq!(ds.mission_rewards_for(None, Some("gillyspilotschool_mission06")).len(), 1, "case does not matter");
        assert!(ds.mission_rewards_for(None, Some("GillysPilotSchool_Mission07_0")).is_empty());
        assert!(ds.mission_rewards_for(Some("nope"), None).is_empty());
    }
}
