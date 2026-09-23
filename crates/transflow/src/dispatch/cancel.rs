use super::*;
// A preflight refusal leaves no running process: cancel the accepted jobs and release
// reservations, retaining the immutable plan as the explanation/retry boundary.
pub(crate) fn unstarted(
    owner: &mut RuntimeOwner,
    rt: &tokio::runtime::Runtime,
    build: BuildId,
) -> Result<()> {
    let plan = repository!(owner, rt, s, s.dispatch_plan(build));
    let binding = ExecutionBinding {
        plan: plan.id.parse().map_err(fail)?,
        source: plan.source.parse().map_err(fail)?,
    };
    let mut jobs = vec![];
    for write in &plan.writes {
        let id = write.job.parse().map_err(fail)?;
        let (_, target, fence) = repository!(owner, rt, s, s.cache_job_context(build, id));
        jobs.push(Job::new(
            id,
            build,
            binding,
            target,
            fence,
            RetryPolicy::default(),
            EventTime(0),
        ));
    }
    repository!(owner, rt, s, s.start_dispatch(&jobs, now()?));
    let mut control =
        tf_exec::cancellation::BuildControl::new(owner, build, jobs.len()).map_err(fail)?;
    rt.block_on(control.request(owner)).map_err(fail)?;
    for job in &mut jobs {
        let old = job.clone();
        job.request_cancel();
        job.finish_canceled(job.fence(), EventTime(0))
            .map_err(fail)?;
        repository!(owner, rt, s, s.persist_execution(&old, job, now()?, 0));
    }
    repository!(
        owner,
        rt,
        s,
        s.finish_dispatch(&jobs, BuildState::Canceled, now()?)
    );
    Ok(())
}
