//! Static cycles in saved automatic schedule dependencies. No imports or provider contact.
use crate::schedules::Error;
use sqlx::SqliteConnection;
use std::collections::{BTreeMap, BTreeSet};
use tf_domain::{ScheduleId, WorkspaceId};
use tf_protocol::schedule::{Definition, Trigger};

pub(crate) async fn validate(
    db: &mut SqliteConnection,
    workspace: WorkspaceId,
    id: ScheduleId,
    candidate: &Definition,
) -> Result<(), Error> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT id,definition_json FROM schedules WHERE workspace_id=? AND deleted_at_us IS NULL AND needs_review=0 AND id!=? ORDER BY id LIMIT 1001")
        .bind(workspace.to_string()).bind(id.to_string()).fetch_all(&mut *db).await.map_err(crate::StoreError::from)?;
    if rows.len() >= 1000 {
        return Err(Error::Limit);
    }
    let mut definitions = BTreeMap::new();
    for (id, raw) in rows {
        let value = serde_json::from_str(&raw).map_err(|_| crate::StoreError::InvalidRequest)?;
        definitions.insert(
            id,
            Definition::decode(&value).map_err(|_| crate::StoreError::InvalidRequest)?,
        );
    }
    definitions.insert(id.to_string(), candidate.clone());
    let mut outputs: BTreeMap<(String, String, String), BTreeSet<String>> = BTreeMap::new();
    for (producer, definition) in &definitions {
        for target in &definition.build.targets {
            outputs
                .entry((
                    target.workspace_id.clone(),
                    target.dataset_id.clone(),
                    definition.build.data_branch.clone(),
                ))
                .or_default()
                .insert(producer.clone());
        }
    }
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut edges = 0usize;
    for (consumer, definition) in &definitions {
        let mut leaves = Vec::new();
        flatten(&definition.trigger, &mut leaves);
        for leaf in leaves {
            let producers = match leaf {
                Trigger::ScheduleSucceeded { schedule_id, .. } => {
                    BTreeSet::from([schedule_id.clone()])
                }
                Trigger::DatasetPublished {
                    dataset, branch, ..
                }
                | Trigger::DatasetHeadChanged {
                    dataset, branch, ..
                } => outputs
                    .get(&(
                        dataset.workspace_id.clone(),
                        dataset.dataset_id.clone(),
                        branch.clone(),
                    ))
                    .cloned()
                    .unwrap_or_default(),
                Trigger::BuildSucceeded {
                    targets, branch, ..
                } => {
                    let mut candidates: Option<BTreeSet<String>> = None;
                    for dataset in targets {
                        let next = outputs
                            .get(&(
                                dataset.workspace_id.clone(),
                                dataset.dataset_id.clone(),
                                branch.clone(),
                            ))
                            .cloned()
                            .unwrap_or_default();
                        candidates = Some(match candidates {
                            None => next,
                            Some(old) => old.intersection(&next).cloned().collect(),
                        });
                    }
                    candidates.unwrap_or_default()
                }
                _ => BTreeSet::new(),
            };
            for producer in producers {
                if graph.entry(producer).or_default().insert(consumer.clone()) {
                    edges += 1;
                }
                if edges > 100_000 {
                    return Err(Error::Limit);
                }
            }
        }
    }
    if let Some(path) = cycle(&graph) {
        return Err(Error::Cycle { path });
    }
    Ok(())
}
fn flatten<'a>(trigger: &'a Trigger, result: &mut Vec<&'a Trigger>) {
    match trigger {
        Trigger::And { children } | Trigger::Or { children } => {
            for child in children {
                flatten(child, result);
            }
        }
        Trigger::Manual | Trigger::Cron { .. } => {}
        leaf => result.push(leaf),
    }
}
fn cycle(graph: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    let mut finished = BTreeSet::new();
    for root in graph.keys() {
        let mut stack = vec![(root.clone(), false)];
        let mut path = Vec::new();
        while let Some((node, leaving)) = stack.pop() {
            if leaving {
                path.pop();
                finished.insert(node);
                continue;
            }
            if let Some(index) = path.iter().position(|current| current == &node) {
                let mut cycle = path[index..].to_vec();
                cycle.push(node);
                return Some(cycle);
            }
            if finished.contains(&node) {
                continue;
            }
            path.push(node.clone());
            stack.push((node.clone(), true));
            if let Some(children) = graph.get(&node) {
                stack.extend(children.iter().rev().map(|child| (child.clone(), false)));
            }
        }
    }
    None
}
