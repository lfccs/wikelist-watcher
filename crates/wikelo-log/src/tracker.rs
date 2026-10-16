//! Per-session mission state machine. Feeds on [`LogEvent`]s and emits
//! higher-level [`TrackerOutput`]s for Wikelo contracts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::parser::{CompletionType, LogEvent};

/// Generator name of every Wikelo contract in DataForge.
pub const WIKELO_GENERATOR: &str = "TheCollector";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MissionState {
    pub mission_id: String,
    pub title: Option<String>,
    pub generator: Option<String>,
    pub debug_name: Option<String>,
    pub contract_id: Option<String>,
    pub accepted_at: Option<String>,
    pub linked_at: Option<String>,
    pub ended_at: Option<String>,
    pub completion: Option<CompletionType>,
    /// (objective_id, item_id, location_hash, state)
    pub objectives: Vec<(String, Option<String>, Option<u64>, String)>,
}

impl MissionState {
    pub fn is_wikelo(&self) -> bool {
        self.generator.as_deref() == Some(WIKELO_GENERATOR)
            || self.debug_name.as_deref().is_some_and(|d| d.starts_with("TheColl"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TrackerOutput {
    SessionStart { at: String, changelist: Option<String> },
    Identity { at: String, nickname: String, geid: String },
    ContractOffered { at: String, debug_name: String },
    HaulingOrderSeen { at: String, debug_name: String, item_id: String },
    /// A Wikelo mission was accepted and linked to its contract.
    MissionLinked { mission: MissionState },
    /// A requirement of an active Wikelo mission changed state (e.g. delivered).
    ObjectiveUpdated { mission: MissionState, objective_id: String, item_id: Option<String>, state: String },
    /// A mission ended (any completion type). Wikelo missions always; other missions
    /// once they were linked to a contract (generator + debug name known).
    MissionEnded { mission: MissionState },
    /// Blueprint notification, correlated to a recently completed Wikelo mission when possible.
    BlueprintReceived { at: String, name: String, mission_id: Option<String> },
    AtEmporium { at: String, location_class: String },
    ItemMoved { at: String, move_type: String, item_class: String, source: String, target: String },
}

#[derive(Debug, Default)]
pub struct MissionTracker {
    missions: HashMap<String, MissionState>,
    pub changelist: Option<String>,
    pub nickname: Option<String>,
    /// Most recent Wikelo completion: (mission_id, at)
    last_complete: Option<(String, String)>,
}

impl MissionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget per-session state (missions), keep identity.
    pub fn reset_session(&mut self) {
        self.missions.clear();
        self.last_complete = None;
    }

    pub fn active_wikelo(&self) -> Vec<&MissionState> {
        let mut v: Vec<&MissionState> = self.missions.values().filter(|m| m.is_wikelo() && m.ended_at.is_none()).collect();
        v.sort_by(|a, b| a.accepted_at.cmp(&b.accepted_at));
        v
    }

    pub fn apply(&mut self, ev: LogEvent) -> Vec<TrackerOutput> {
        let mut out = Vec::new();
        match ev {
            LogEvent::SessionStart { at, changelist } => {
                self.reset_session();
                if changelist.is_some() {
                    self.changelist = changelist.clone();
                }
                out.push(TrackerOutput::SessionStart { at, changelist });
            }
            LogEvent::Identity { at, nickname, geid } => {
                self.nickname = Some(nickname.clone());
                out.push(TrackerOutput::Identity { at, nickname, geid });
            }
            LogEvent::ContractOffered { at, debug_name, .. } => {
                out.push(TrackerOutput::ContractOffered { at, debug_name });
            }
            LogEvent::HaulingOrderSeen { at, debug_name, item_id, .. } => {
                out.push(TrackerOutput::HaulingOrderSeen { at, debug_name, item_id });
            }
            LogEvent::MissionAccepted { at, mission_id, title } => {
                let m = self.missions.entry(mission_id.clone()).or_insert_with(|| MissionState { mission_id, ..Default::default() });
                if m.accepted_at.is_none() {
                    m.accepted_at = Some(at);
                }
                m.title.get_or_insert(title);
                // Already linked (marker came first)?
                if m.is_wikelo() && m.linked_at.is_some() && m.ended_at.is_none() {
                    out.push(TrackerOutput::MissionLinked { mission: m.clone() });
                }
            }
            LogEvent::MissionMarker { at, mission_id, generator, debug_name, contract_id, objective_id, item_id, location_hash } => {
                let m = self.missions.entry(mission_id.clone()).or_insert_with(|| MissionState { mission_id, ..Default::default() });
                let was_linked = m.linked_at.is_some();
                m.generator.get_or_insert(generator);
                m.debug_name.get_or_insert(debug_name);
                if contract_id.is_some() {
                    m.contract_id = contract_id;
                }
                if let Some(obj) = objective_id
                    && !m.objectives.iter().any(|(o, ..)| *o == obj)
                {
                    m.objectives.push((obj, item_id, location_hash, "ACTIVE".into()));
                }
                if m.linked_at.is_none() {
                    m.linked_at = Some(at);
                }
                if !was_linked && m.is_wikelo() && m.ended_at.is_none() {
                    out.push(TrackerOutput::MissionLinked { mission: m.clone() });
                }
            }
            LogEvent::ObjectiveState { at: _, mission_id, objective_id, state, item_id, location_hash } => {
                if let Some(m) = self.missions.get_mut(&mission_id) {
                    match m.objectives.iter_mut().find(|(o, ..)| *o == objective_id) {
                        Some(entry) => {
                            entry.3 = state.clone();
                            if entry.1.is_none() {
                                entry.1 = item_id.clone();
                            }
                            if entry.2.is_none() {
                                entry.2 = location_hash;
                            }
                        }
                        None => m.objectives.push((objective_id.clone(), item_id.clone(), location_hash, state.clone())),
                    }
                    if m.is_wikelo() {
                        out.push(TrackerOutput::ObjectiveUpdated { mission: m.clone(), objective_id, item_id, state });
                    }
                }
            }
            LogEvent::EndMission { at, mission_id, nickname, completion, .. } => {
                if self.nickname.is_none() && !nickname.is_empty() {
                    self.nickname = Some(nickname);
                }
                if let Some(m) = self.missions.get_mut(&mission_id) {
                    if m.ended_at.is_some() {
                        return out; // duplicate end line
                    }
                    m.ended_at = Some(at.clone());
                    m.completion = Some(completion.clone());
                    if m.is_wikelo() && completion.is_complete() {
                        self.last_complete = Some((mission_id.clone(), at));
                    }
                    // Every linked mission reports its end: other contract givers pay items
                    // the tracker cares about (scrip), and the server decides what to do.
                    if m.is_wikelo() || m.linked_at.is_some() {
                        out.push(TrackerOutput::MissionEnded { mission: m.clone() });
                    }
                }
            }
            LogEvent::BlueprintReceived { at, name } => {
                let mission_id = self
                    .last_complete
                    .as_ref()
                    .filter(|(_, when)| within_seconds(when, &at, 120))
                    .map(|(id, _)| id.clone());
                out.push(TrackerOutput::BlueprintReceived { at, name, mission_id });
            }
            LogEvent::AtEmporium { at, location_class, .. } => {
                out.push(TrackerOutput::AtEmporium { at, location_class });
            }
            LogEvent::ItemMoved { at, move_type, item_class, source, target, .. } => {
                out.push(TrackerOutput::ItemMoved { at, move_type, item_class, source, target });
            }
        }
        out
    }
}

/// True when `later` is within `secs` seconds after `earlier` (both RFC 3339).
fn within_seconds(earlier: &str, later: &str, secs: i64) -> bool {
    let fmt = &time::format_description::well_known::Rfc3339;
    match (time::OffsetDateTime::parse(earlier, fmt), time::OffsetDateTime::parse(later, fmt)) {
        (Ok(a), Ok(b)) => {
            let d = (b - a).whole_seconds();
            (0..=secs).contains(&d)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::LineParser;

    fn feed(lines: &[&str]) -> Vec<TrackerOutput> {
        let p = LineParser::new();
        let mut t = MissionTracker::new();
        let mut out = Vec::new();
        for l in lines {
            if let Some(ev) = p.parse(l) {
                out.extend(t.apply(ev));
            }
        }
        out
    }

    #[test]
    fn accept_link_complete_flow() {
        let out = feed(&[
            "<2026-10-12T18:44:30.000Z> [Notice] <SHUDEvent_OnNotification> Added notification \"Contract Accepted:  Want Polaris? Need something special.: \" [1] to queue. New queue size: 1, MissionId: [c8c64cab-0eda-49eb-8f2b-538e2d4db547], ObjectiveId: [] [Team_CoreGameplayFeatures][Missions][Comms]",
            "<2026-10-12T18:44:32.784Z> [Notice] <CLocalMissionPhaseMarker::CreateMarker> Creating objective marker: missionId [c8c64cab-0eda-49eb-8f2b-538e2d4db547], generator name [TheCollector], contract [TheCollector_Favours_PolarisParts], contractDefinitionId[82a0d506-def3-4ab5-b0e0-8833c7102f34], objectiveId [dropoff_f94ac2dc_0], markerEntityId [1], zoneHostId [1], position [x: 0, y: 0, z: 0] [Team_MissionFeatures][Missions]",
            "<2026-10-12T18:44:32.785Z> [Notice] <CLocalMissionPhaseMarker::CreateMarker> Creating objective marker: missionId [c8c64cab-0eda-49eb-8f2b-538e2d4db547], generator name [TheCollector], contract [TheCollector_Favours_PolarisParts], contractDefinitionId[82a0d506-def3-4ab5-b0e0-8833c7102f34], objectiveId [dropoff_f94ac2dc_0], markerEntityId [2], zoneHostId [1], position [x: 0, y: 0, z: 0] [Team_MissionFeatures][Missions]",
            "<2026-10-12T18:46:46.724Z> [Notice] <EndMission> Ending mission for player. MissionId[c8c64cab-0eda-49eb-8f2b-538e2d4db547] Player[felcs] PlayerId[204354536146] CompletionType[Complete] Reason[Mission Ended] [Team_MissionFeatures][Missions]",
            "<2026-10-12T18:46:46.724Z> [Notice] <EndMission> Ending mission for player. MissionId[c8c64cab-0eda-49eb-8f2b-538e2d4db547] Player[felcs] PlayerId[204354536146] CompletionType[Complete] Reason[Mission Ended] [Team_MissionFeatures][Missions]",
            "<2026-10-12T18:46:50.000Z> [Notice] <SHUDEvent_OnNotification> Added notification \"Received Blueprint: Something: \" [2] to queue. New queue size: 1, MissionId: [00000000-0000-0000-0000-000000000000], ObjectiveId: [] [Team_CoreGameplayFeatures][Missions][Comms]",
        ]);
        assert_eq!(out.len(), 3, "{out:?}");
        match &out[0] {
            TrackerOutput::MissionLinked { mission } => {
                assert_eq!(mission.title.as_deref(), Some("Want Polaris? Need something special."));
                assert_eq!(mission.contract_id.as_deref(), Some("82a0d506-def3-4ab5-b0e0-8833c7102f34"));
                assert_eq!(mission.accepted_at.as_deref(), Some("2026-10-12T18:44:30.000Z"));
            }
            o => panic!("{o:?}"),
        }
        match &out[1] {
            TrackerOutput::MissionEnded { mission } => {
                assert_eq!(mission.completion, Some(CompletionType::Complete));
                assert_eq!(mission.ended_at.as_deref(), Some("2026-10-12T18:46:46.724Z"));
            }
            o => panic!("{o:?}"),
        }
        match &out[2] {
            TrackerOutput::BlueprintReceived { mission_id, .. } => {
                assert_eq!(mission_id.as_deref(), Some("c8c64cab-0eda-49eb-8f2b-538e2d4db547"));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn non_wikelo_missions_only_report_their_end() {
        let out = feed(&[
            "<2026-10-12T17:50:15.036Z> [Notice] <SHUDEvent_OnNotification> Added notification \"Contract Accepted:  RSI Disc. Month: Important Supply Haul: \" [25] to queue. New queue size: 2, MissionId: [7a2e2cf3-1c1b-457f-81fe-725b990fc0b3], ObjectiveId: [] [Team_CoreGameplayFeatures][Missions][Comms]",
            "<2026-10-12T17:50:16.000Z> [Notice] <CLocalMissionPhaseMarker::CreateMarker> Creating objective marker: missionId [7a2e2cf3-1c1b-457f-81fe-725b990fc0b3], generator name [RSI_Discovery], contract [RSI_Disc_Haul], contractDefinitionId[11111111-1111-1111-1111-111111111111], objectiveId [x], markerEntityId [1], zoneHostId [1], position [x: 0, y: 0, z: 0] [Team_MissionFeatures][Missions]",
            "<2026-10-12T21:46:30.460Z> [Notice] <EndMission> Ending mission for player. MissionId[7a2e2cf3-1c1b-457f-81fe-725b990fc0b3] Player[felcs] PlayerId[204354536146] CompletionType[Complete] Reason[Mission Ended] [Team_MissionFeatures][Missions]",
        ]);
        assert_eq!(out.len(), 1, "{out:?}");
        match &out[0] {
            TrackerOutput::MissionEnded { mission } => {
                assert_eq!(mission.generator.as_deref(), Some("RSI_Discovery"));
                assert!(!mission.is_wikelo());
                assert!(mission.completion.as_ref().unwrap().is_complete());
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn session_start_resets_missions() {
        let p = LineParser::new();
        let mut t = MissionTracker::new();
        t.apply(p.parse("<2026-10-12T18:44:32.784Z> [Notice] <CLocalMissionPhaseMarker::CreateMarker> Creating objective marker: missionId [c8c64cab-0eda-49eb-8f2b-538e2d4db547], generator name [TheCollector], contract [TheCollector_Intro], contractDefinitionId[028d7a46-bd97-4e32-a5bf-0b1775f05e1c], objectiveId [o], markerEntityId [1], zoneHostId [1], position [x: 0, y: 0, z: 0] [Team_MissionFeatures][Missions]").unwrap());
        assert_eq!(t.active_wikelo().len(), 1);
        t.apply(p.parse("<2026-10-13T21:34:02.356Z> BackupNameAttachment=\" Build(12913470) 13 Oct 26 (18 33 55)\"  -- used by backup system").unwrap());
        assert!(t.active_wikelo().is_empty());
        assert_eq!(t.changelist.as_deref(), Some("12913470"));
    }
}
