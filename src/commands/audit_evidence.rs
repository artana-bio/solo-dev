//! Independent recomputation of persisted verification evidence.

use crate::{
    commands::acceptance::acceptance_for,
    control::repository::ControlRepository,
    domain::{
        card::CardRecord,
        ids::ReceiptId,
        integration::{ClaimClassification, IntegrationRecord, VerificationRecord},
    },
    error::{ErrorCode, HarnessError},
    git::command::{GitScope, run_ok},
    runner::receipt::Receipt,
};

use super::audit::{Discrepancy, commit_exists};

#[allow(clippy::too_many_lines, clippy::type_complexity)]
pub(crate) fn cross_check_verification(
    control: &ControlRepository,
    config: &crate::config::ProjectConfig,
    integration: &IntegrationRecord,
    verification: &VerificationRecord,
    found: &mut Vec<Discrepancy>,
) -> Result<Vec<(Option<String>, String, ClaimClassification, Vec<String>)>, HarnessError> {
    let mut evidence_invalid = false;
    if integration.landing_sha.as_deref() != Some(verification.landing_sha.as_str()) {
        evidence_invalid = true;
        found.push(Discrepancy {
            subject: format!("verification {}", integration.integration_id),
            claim: format!("landing SHA {}", verification.landing_sha),
            found: format!("integration records {:?}", integration.landing_sha),
        });
    }
    if integration.integration_tree.as_deref() != Some(verification.landing_tree.as_str()) {
        evidence_invalid = true;
        found.push(Discrepancy {
            subject: format!("verification {}", integration.integration_id),
            claim: format!("landing tree {}", verification.landing_tree),
            found: format!("integration records {:?}", integration.integration_tree),
        });
    }
    if commit_exists(&config.repository, &verification.landing_sha) {
        match run_ok(
            &GitScope::work_tree(&config.repository),
            [
                "rev-parse",
                &format!("{}^{{tree}}", verification.landing_sha),
            ],
        ) {
            Ok(tree) if tree.trimmed_stdout() != verification.landing_tree => {
                evidence_invalid = true;
                found.push(Discrepancy {
                    subject: format!("verification {}", integration.integration_id),
                    claim: format!("landing object tree {}", verification.landing_tree),
                    found: format!("Git reports {}", tree.trimmed_stdout()),
                });
            }
            Err(error) => {
                // A missing/unreadable landing object cannot support a
                // mechanically checked claim, even if the stored strings
                // happen to agree.
                //
                // The error is retained below as a discrepancy; this flag
                // makes the classification reflect it as well.
                evidence_invalid = true;
                found.push(Discrepancy {
                    subject: format!("verification {}", integration.integration_id),
                    claim: "landing object exists".to_owned(),
                    found: error.to_string(),
                });
            }
            _ => {}
        }
    } else {
        evidence_invalid = true;
        found.push(Discrepancy {
            subject: format!("verification {}", integration.integration_id),
            claim: format!("landing commit {} exists", verification.landing_sha),
            found: "landing commit is absent".to_owned(),
        });
    }
    let mut receipts = std::collections::BTreeMap::new();
    let mut invalid_receipts = std::collections::BTreeSet::new();
    for id in &verification.receipt_ids {
        let parsed: ReceiptId = match id.parse() {
            Ok(value) => value,
            Err(error) => {
                found.push(Discrepancy {
                    subject: format!("verification {}", integration.integration_id),
                    claim: format!("receipt {id}"),
                    found: format!("malformed id: {error}"),
                });
                continue;
            }
        };
        let path = Receipt::relative_path(&parsed);
        match control.read(&path).and_then(|raw| {
            serde_json::from_str::<Receipt>(&raw).map_err(|source| HarnessError::Control {
                reason: source.to_string(),
                code: ErrorCode::InternalControlCorrupt,
            })
        }) {
            Ok(receipt) => {
                let mut valid = receipt.integration_id.as_ref()
                    == Some(&integration.integration_id)
                    && receipt.cycle_id == integration.cycle_id
                    && receipt.card_id.is_none()
                    && receipt.evaluated_sha == verification.landing_sha
                    && receipt.passed
                    && receipt.worktree_clean == Some(true);
                if receipt.reuse_material().is_err() {
                    valid = false;
                }
                if !valid {
                    evidence_invalid = true;
                    invalid_receipts.insert(id.clone());
                    found.push(Discrepancy {
                        subject: format!("receipt {id}"),
                        claim: "passing integration receipt at the verified landing SHA".to_owned(),
                        found: format!(
                            "owner={:?}, cycle={}, sha={}, passed={}, clean={:?}",
                            receipt.integration_id,
                            receipt.cycle_id,
                            receipt.evaluated_sha,
                            receipt.passed,
                            receipt.worktree_clean
                        ),
                    });
                }
                receipts.insert(id.clone(), (receipt, valid));
            }
            Err(error) => {
                invalid_receipts.insert(id.clone());
                found.push(Discrepancy {
                    subject: format!("receipt {id}"),
                    claim: "receipt exists and is readable".to_owned(),
                    found: error.to_string(),
                });
            }
        }
    }
    let bound_policy = config
        .final_authorization_policy
        .as_ref()
        .map(crate::config::FinalAuthorizationPolicy::digest)
        .transpose()?;
    if let Some(acceptance) = acceptance_for(control, &integration.integration_id)? {
        let integration_digest = integration.digest()?;
        if acceptance.landing_sha != verification.landing_sha {
            evidence_invalid = true;
            found.push(Discrepancy {
                subject: format!("acceptance {}", acceptance.acceptance_id),
                claim: "accepted landing SHA".to_owned(),
                found: format!("{} != {}", acceptance.landing_sha, verification.landing_sha),
            });
        }
        if acceptance.integration_record_digest != integration_digest {
            evidence_invalid = true;
            found.push(Discrepancy {
                subject: format!("acceptance {}", acceptance.acceptance_id),
                claim: "accepted integration record digest".to_owned(),
                found: format!(
                    "recorded={}, current={}",
                    acceptance.integration_record_digest, integration_digest
                ),
            });
        }
        let mut accepted_receipts = acceptance.receipt_ids.clone();
        let mut verified_receipts = verification.receipt_ids.clone();
        accepted_receipts.sort();
        verified_receipts.sort();
        if accepted_receipts != verified_receipts {
            evidence_invalid = true;
            found.push(Discrepancy {
                subject: format!("acceptance {}", acceptance.acceptance_id),
                claim: "accepted verification receipt set".to_owned(),
                found: format!("recorded={accepted_receipts:?}, current={verified_receipts:?}"),
            });
        }
        if acceptance.final_authorization_policy_digest != bound_policy {
            evidence_invalid = true;
            found.push(Discrepancy {
                subject: format!("acceptance {}", acceptance.acceptance_id),
                claim: "authorization policy digest bound at acceptance".to_owned(),
                found: format!(
                    "recorded={:?}, current={:?}",
                    acceptance.final_authorization_policy_digest, bound_policy
                ),
            });
        }
    }
    let mut claims = Vec::new();
    for check in &verification.invariants {
        let mut expected_receipts = Vec::new();
        let proof = integration.members.iter().find_map(|member| {
            let path = CardRecord::relative_path(&member.card_id, member.card_revision);
            let raw = match control.read(&path) {
                Ok(raw) => raw,
                Err(error) => {
                    found.push(Discrepancy {
                        subject: member.card_id.to_string(),
                        claim: format!("card revision {} proof map", member.card_revision),
                        found: error.to_string(),
                    });
                    return None;
                }
            };
            let card: CardRecord = match serde_json::from_str(&raw) {
                Ok(card) => card,
                Err(error) => {
                    found.push(Discrepancy {
                        subject: member.card_id.to_string(),
                        claim: format!("card revision {} proof map", member.card_revision),
                        found: error.to_string(),
                    });
                    return None;
                }
            };
            let Some(map) = card.proof_map else {
                found.push(Discrepancy {
                    subject: member.card_id.to_string(),
                    claim: "proof map".to_owned(),
                    found: "missing proof map".to_owned(),
                });
                return None;
            };
            map.entries
                .into_iter()
                .find(|entry| entry.id.as_deref() == check.proof_entry_id.as_deref())
        });
        let Some(proof) = proof else {
            found.push(Discrepancy {
                subject: check
                    .proof_entry_id
                    .clone()
                    .unwrap_or_else(|| check.invariant.clone()),
                claim: "stable proof entry with declared oracle".to_owned(),
                found: "proof entry is missing".to_owned(),
            });
            claims.push((
                check.proof_entry_id.clone(),
                check.invariant.clone(),
                ClaimClassification::NotTested,
                Vec::new(),
            ));
            continue;
        };
        let Some(oracle) = proof.gate_oracle else {
            found.push(Discrepancy {
                subject: check
                    .proof_entry_id
                    .clone()
                    .unwrap_or_else(|| check.invariant.clone()),
                claim: "declared proof oracle".to_owned(),
                found: "oracle binding is missing".to_owned(),
            });
            claims.push((
                check.proof_entry_id.clone(),
                check.invariant.clone(),
                ClaimClassification::NotTested,
                Vec::new(),
            ));
            continue;
        };
        for (id, (receipt, valid)) in &receipts {
            if *valid
                && receipt.gate_id == oracle
                && receipt.evaluated_sha == verification.landing_sha
            {
                expected_receipts.push(id.clone());
            }
        }
        expected_receipts.sort();
        let mut observed = check.observed_receipt_ids.clone();
        observed.sort();
        let forged_or_missing = observed != expected_receipts
            || observed.iter().any(|id| invalid_receipts.contains(id));
        if forged_or_missing {
            found.push(Discrepancy {
                subject: check
                    .proof_entry_id
                    .clone()
                    .unwrap_or_else(|| check.invariant.clone()),
                claim: format!("receipt coverage for oracle {oracle}"),
                found: format!("observed={observed:?}, recomputed={expected_receipts:?}"),
            });
        }
        let claim_invalid_receipt = observed.iter().any(|id| invalid_receipts.contains(id));
        let valid = !evidence_invalid
            && !expected_receipts.is_empty()
            && observed == expected_receipts
            && !claim_invalid_receipt
            && check.machine_checked;
        if check.machine_checked && !valid {
            found.push(Discrepancy {
                subject: check
                    .proof_entry_id
                    .clone()
                    .unwrap_or_else(|| check.invariant.clone()),
                claim: "machine_checked".to_owned(),
                found: "the persisted flag is not supported by exact receipt coverage".to_owned(),
            });
        }
        claims.push((
            check.proof_entry_id.clone(),
            check.invariant.clone(),
            if valid {
                ClaimClassification::MachineChecked
            } else if forged_or_missing || claim_invalid_receipt || evidence_invalid {
                ClaimClassification::Failed
            } else {
                ClaimClassification::NotTested
            },
            expected_receipts,
        ));
    }
    let _ = config;
    Ok(claims)
}
