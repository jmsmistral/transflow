//! Serialized execution decisions and durable observations.
use super::*;
impl Coordinator<'_> {
    pub(super) fn time(&self) -> EventTime {
        EventTime(self.started.elapsed().as_millis() as u64)
    }
    pub(super) fn transition(
        &mut self,
        id: JobId,
        edit: impl FnOnce(
            &mut Job,
            EventTime,
        ) -> std::result::Result<(), tf_domain::execution::StateError>,
    ) -> Result<()> {
        self.transition_at(id, Instant::now(), edit)
    }
    pub(super) fn transition_at(
        &mut self,
        id: JobId,
        instant: Instant,
        edit: impl FnOnce(
            &mut Job,
            EventTime,
        ) -> std::result::Result<(), tf_domain::execution::StateError>,
    ) -> Result<()> {
        let old = self
            .prepared
            .jobs
            .get(&id)
            .ok_or_else(|| fail("unknown job"))?
            .clone();
        let mut next = old.clone();
        edit(
            &mut next,
            EventTime(
                instant
                    .checked_duration_since(self.started)
                    .ok_or_else(|| fail("invalid phase clock"))?
                    .as_millis()
                    .try_into()
                    .map_err(fail)?,
            ),
        )
        .map_err(fail)?;
        let duration = self
            .phase_start
            .get(&id)
            .map(|t| instant.duration_since(*t).as_nanos())
            .unwrap_or(0)
            .try_into()
            .map_err(fail)?;
        repository!(
            self.owner,
            self.rt,
            s,
            s.persist_execution(
                &old,
                &next,
                now()?
                    .checked_sub(instant.elapsed().as_micros().try_into().map_err(fail)?)
                    .ok_or_else(|| fail("invalid phase clock"))?,
                duration
            )
        );
        self.phase_start.insert(id, instant);
        self.prepared.jobs.insert(id, next);
        Ok(())
    }
    pub(super) fn advance(&mut self, id: JobId, phase: Phase) -> Result<()> {
        self.transition(id, |j, t| {
            j.advance(
                j.attempts()
                    .last()
                    .ok_or(tf_domain::execution::StateError::InvalidEvidence)?
                    .id(),
                j.fence(),
                phase,
                t,
            )
        })
    }
    pub(super) fn materialization(
        &mut self,
        event: tf_exec::supervisor::WorkerPhase,
    ) -> Result<()> {
        if event.name != "materializing" {
            return Ok(());
        }
        let id = self
            .prepared
            .jobs
            .values()
            .find(|j| j.attempts().last().is_some_and(|a| a.id() == event.attempt))
            .ok_or_else(|| fail("unregistered worker phase"))?
            .id();
        self.transition_at(id, event.observed, |j, t| {
            j.advance(event.attempt, j.fence(), Phase::Materializing, t)
        })
    }
    pub(super) fn observe(&mut self, id: JobId, event: worker::Observation) -> Result<()> {
        match event {
            worker::Observation::Phase(p) => self.advance(
                id,
                match p {
                    tf_exec::timing::Phase::InputValidation => Phase::ValidatingInputs,
                    tf_exec::timing::Phase::Transform => Phase::Running,
                    tf_exec::timing::Phase::OutputValidation => Phase::ValidatingOutputs,
                    _ => return Err(fail("unexpected execution phase")),
                },
            ),
            event => {
                let a = self
                    .active
                    .get(&id)
                    .ok_or_else(|| fail("missing active job"))?;
                let job = &self.prepared.jobs[&id];
                let policy = json!({"allowed_columns":[],"sensitive_columns":[],"max_rows":20});
                let c = tf_store::check_evidence::Context {
                    job,
                    attempt: job
                        .attempts()
                        .last()
                        .ok_or_else(|| fail("missing attempt"))?
                        .id(),
                    contract: &a.contract,
                    at_us: now()?,
                    consumer_definition: &a.consumer,
                    sample_policy: &policy,
                };
                match event {
                    worker::Observation::Evaluation {
                        result,
                        manifest,
                        samples,
                    } => {
                        repository!(
                            self.owner,
                            self.rt,
                            s,
                            s.record_check_evaluation(&c, &result, &manifest, &samples)
                        );
                    }
                    worker::Observation::Rejected(digest) => {
                        let artifact = ArtifactStore::open(self.owner.workspace_root())
                            .map_err(fail)?
                            .verify(digest)
                            .map_err(fail)?;
                        repository!(
                            self.owner,
                            self.rt,
                            s,
                            s.record_failed_check_candidate(&c, &artifact)
                        );
                    }
                    _ => unreachable!(),
                }
                Ok(())
            }
        }
    }
    pub(super) fn cancel_job(&mut self, id: JobId) -> Result<()> {
        self.transition(id, |j, t| {
            j.request_cancel();
            j.finish_canceled(j.fence(), t)
        })
    }
    pub(super) fn failure(&mut self, id: JobId) -> Result<()> {
        use tf_domain::{
            diagnostic::{Diagnostic, DiagnosticCode, Redactor},
            execution::{FailureClass, FailureEvidence},
        };
        let r = Redactor::default();
        let diagnostic = Diagnostic::new(
            DiagnosticCode::OperationFailed,
            r.text("Dataset execution failed").map_err(fail)?,
            r.text("The worker, checks or publication did not complete successfully.")
                .map_err(fail)?,
            r.text(
                "Inspect the retained attempt and check evidence before submitting another build.",
            )
            .map_err(fail)?,
        );
        self.transition(id, |j, t| {
            j.fail(
                j.attempts()
                    .last()
                    .ok_or(tf_domain::execution::StateError::InvalidEvidence)?
                    .id(),
                j.fence(),
                FailureEvidence {
                    class: FailureClass::Other,
                    diagnostic,
                },
                t,
            )
        })
    }
    pub(super) fn contract(&mut self, id: JobId) -> Result<(tf_store::cache::Request, bool)> {
        let j = &self.prepared.jobs[&id];
        let (_, resolved) = &self.prepared.declarations[&id];
        let mut checks = vec![];
        let mut owned = self.rt.block_on(self.owner.open_store()).map_err(fail)?;
        let s = owned.repository().map_err(fail)?;
        for input in resolved["inputs"]
            .as_array()
            .ok_or_else(|| fail("missing inputs"))?
        {
            checks.extend(
                self.rt
                    .block_on(
                        s.register_check_definitions(
                            j.target().dataset.workspace_id(),
                            j.fence().session,
                            Some(text(input, "alias")?),
                            input["checks"]
                                .as_array()
                                .ok_or_else(|| fail("missing checks"))?,
                        ),
                    )
                    .map_err(fail)?,
            );
        }
        checks.extend(
            self.rt
                .block_on(
                    s.register_check_definitions(
                        j.target().dataset.workspace_id(),
                        j.fence().session,
                        None,
                        resolved["output"]["checks"]
                            .as_array()
                            .ok_or_else(|| fail("missing checks"))?,
                    ),
                )
                .map_err(fail)?,
        );
        self.rt.block_on(owned.close()).map_err(fail)?;
        let semantics = crate::cache::Semantics {
            writer: json!({"normalization":"parquet-normalized-v1","compression":"zstd","row_group_size":"8192"}),
            evaluation_us: if !self.prepared.contextual {
                None
            } else {
                Some(self.prepared.plan.created_us)
            },
            secret_versions: BTreeMap::new(),
            checks,
        };
        match self
            .rt
            .block_on(crate::cache::prepare_inner(
                self.owner,
                j.build(),
                id,
                semantics,
                now()?,
            ))
            .map_err(fail)?
        {
            crate::cache::Decision::Pending => {
                Err(fail("ready job still has an unresolved accepted parent"))
            }
            crate::cache::Decision::Execute { request, .. } => Ok((*request, false)),
            crate::cache::Decision::Ready(r) => Ok((*r, true)),
        }
    }
    pub(super) fn cache(
        &mut self,
        r: &tf_store::cache::Request,
        artifacts: &ArtifactStore,
    ) -> Result<bool> {
        let lookup = repository!(
            self.owner,
            self.rt,
            s,
            s.lookup_cache(r, new_id()?, 3_600_000_000)
        );
        let receipt = match lookup {
            tf_store::cache::Lookup::Miss => return Ok(false),
            tf_store::cache::Lookup::Reused(r) => r,
            tf_store::cache::Lookup::Candidate(c) => {
                let checked = artifacts.verify(c.artifact()).map_err(fail);
                let adopted = match checked {
                    Ok(v) => {
                        let mut owned = self.rt.block_on(self.owner.open_store()).map_err(fail)?;
                        let result =
                            self.rt
                                .block_on(owned.repository().map_err(fail)?.adopt_cache(
                                    &c,
                                    &v,
                                    now()?,
                                ))
                                .map_err(fail);
                        self.rt.block_on(owned.close()).map_err(fail)?;
                        result
                    }
                    Err(e) => Err(e),
                };
                repository!(self.owner, self.rt, s, s.release_read(c.lease()));
                adopted?
            }
        };
        self.transition(r.job, |j, t| j.cache(receipt.version, j.fence(), t))?;
        Ok(true)
    }
    pub(super) fn finish(
        &mut self,
        id: JobId,
        c: tf_exec::lifecycle::Completion<'_>,
        canceled: bool,
    ) -> Result<bool> {
        let j = &self.prepared.jobs[&id];
        let a = &self.active[&id];
        let summary = worker::report(&c);
        repository!(
            self.owner,
            self.rt,
            s,
            s.record_execution_report(
                j,
                &summary,
                a.directory
                    .to_str()
                    .ok_or_else(|| fail("invalid log path"))?
            )
        );
        let mut success = false;
        if let Ok(approved) = c.outcome {
            let mut next = j.clone();
            let attempt = next
                .attempts()
                .last()
                .ok_or_else(|| fail("missing attempt"))?
                .id();
            let intent = next
                .prepare_publication(
                    attempt,
                    next.fence(),
                    tf_domain::VersionId::from_bytes(*new_id()?.as_bytes()),
                    approved
                        .digest()
                        .map_err(fail)?
                        .hex()
                        .parse()
                        .map_err(fail)?,
                    self.time(),
                )
                .map_err(fail)?;
            let request = tf_store::publication::PublicationRequest {
                intent: intent.clone(),
                contract: a.contract.clone(),
                at_us: now()?,
            };
            let previous = j.clone();
            let mut committing_at = None;
            let duration = self.phase_start[&id]
                .elapsed()
                .as_nanos()
                .try_into()
                .map_err(fail)?;
            let publication = tf_exec::lifecycle::publish_observed(
                self.owner,
                self.rt,
                request,
                approved,
                || now().map_err(|_| tf_exec::lifecycle::Error::Contract),
                |store| {
                    self.rt.block_on(store.persist_execution(
                        &previous,
                        &next,
                        now().map_err(|_| tf_exec::lifecycle::Error::Contract)?,
                        duration,
                    ))?;
                    committing_at = Some(Instant::now());
                    Ok(())
                },
            );
            if let Some(at) = committing_at {
                self.prepared.jobs.insert(id, next.clone());
                self.phase_start.insert(id, at);
            }
            match publication {
                Ok(_) => {
                    next.commit(
                        &intent,
                        next.fence(),
                        next.target().expected_generation,
                        self.time(),
                    )
                    .map_err(fail)?;
                    let previous = &self.prepared.jobs[&id];
                    repository!(
                        self.owner,
                        self.rt,
                        s,
                        s.persist_execution(
                            previous,
                            &next,
                            now()?,
                            self.phase_start[&id]
                                .elapsed()
                                .as_nanos()
                                .try_into()
                                .map_err(fail)?
                        )
                    );
                    self.prepared.jobs.insert(id, next);
                    success = true;
                }
                Err(error) => {
                    let mut report = summary.clone();
                    report["publication_error"] = json!(error.to_string());
                    report["stage"] = json!("publication");
                    repository!(
                        self.owner,
                        self.rt,
                        s,
                        s.record_execution_report(
                            &self.prepared.jobs[&id],
                            &report,
                            a.directory
                                .to_str()
                                .ok_or_else(|| fail("invalid log path"))?
                        )
                    );
                }
            }
        }
        if !success {
            let canceled = canceled
                || repository!(
                    self.owner,
                    self.rt,
                    s,
                    s.dispatch_canceled(self.prepared.jobs[&id].build())
                );
            if canceled {
                self.cancel_job(id)?
            } else {
                self.failure(id)?
            }
        }
        let active = self
            .active
            .remove(&id)
            .ok_or_else(|| fail("missing active job"))?;
        for lease in active.leases {
            repository!(self.owner, self.rt, s, s.release_read(&lease));
        }
        Ok(success)
    }
    pub(super) fn renew(&mut self) -> Result<()> {
        let plan = &self.prepared.plan;
        let job = self
            .prepared
            .jobs
            .values()
            .next()
            .ok_or_else(|| fail("empty build"))?;
        repository!(
            self.owner,
            self.rt,
            s,
            s.renew_dispatch_boundaries(
                job.target().dataset.workspace_id(),
                job.fence().session,
                plan,
                now()?
            )
        );
        for active in self.active.values_mut() {
            for lease in &mut active.leases {
                *lease = repository!(
                    self.owner,
                    self.rt,
                    s,
                    s.renew_read(lease, now()?, 3_600_000_000)
                );
            }
        }
        Ok(())
    }
}
