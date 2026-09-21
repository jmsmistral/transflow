//! Thin public plan/why/lineage adapters over captured shared services.
use crate::why::identity;
use crate::{
    build_plan::{self, Error, failure},
    graph_query,
};
use clap::{Arg, ArgAction, ArgMatches, Command};
use serde_json::{Value, json};
use std::path::Path;
use tf_catalog::{RegistrySnapshot, git, traversal};
use tf_domain::{BranchName, BranchSelector, diagnostic::Redactor};
use tf_exec::ownership::{CoordinatorMode, RuntimeOwner};

pub(crate) fn commands() -> Vec<Command> {
    ["plan", "why", "upstream", "downstream"]
        .into_iter()
        .map(|name| {
            let mut cmd =
                Command::new(name)
                    .disable_help_flag(true)
                    .about(match name {
                        "plan" => "Save a guarded nonexecuting draft",
                        "why" => "Explain freshness and prospective selection",
                        "upstream" => "Show all declared ancestors unless depth is supplied",
                        _ => "Show all declared descendants unless depth is supplied",
                    })
                    .arg(Arg::new("python").long("python").value_name("EXECUTABLE"))
                    .arg(
                        Arg::new("branch")
                            .long("branch")
                            .value_parser(clap::value_parser!(BranchName)),
                    )
                    .arg(Arg::new("git-ref").long("git-ref").requires("branch").help(
                        "Capture a Git ref without changing the checkout; requires --branch",
                    ));
            if matches!(name, "upstream" | "downstream") {
                cmd = cmd.arg(Arg::new("reference").required_unless_present("help")).arg(
                    Arg::new("depth")
                        .long("depth")
                        .value_parser(traversal::parse_depth)
                        .help(
                            "Maximum shortest hop distance; omitted returns every reachable node",
                        ),
                );
            } else {
                cmd = cmd
                    .arg(
                        Arg::new("targets")
                            .num_args(if name == "why" { 1..=1 } else { 1..=usize::MAX })
                            .action(ArgAction::Append)
                            .required_unless_present_any(["target", "help"]),
                    )
                    .arg(
                        Arg::new("target")
                            .long("target")
                            .action(ArgAction::Append)
                            .help("Alternative or additional exact target reference"),
                    )
                    .arg(
                        Arg::new("mode")
                            .long("mode")
                            .value_parser(["full", "selected", "between"])
                            .default_value("full"),
                    )
                    .arg(
                        Arg::new("boundary-policy")
                            .long("boundary-policy")
                            .value_parser(["require_available", "require_current"])
                            .default_value("require_available"),
                    )
                    .arg(Arg::new("force").long("force").action(ArgAction::SetTrue))
                    .arg(
                        Arg::new("no-fallback")
                            .long("no-fallback")
                            .action(ArgAction::SetTrue)
                            .conflicts_with("fallback"),
                    )
                    .arg(
                        Arg::new("timeout-seconds")
                            .long("timeout-seconds")
                            .value_parser(traversal::parse_depth),
                    )
                    .arg(
                        Arg::new("validation-timeout-seconds")
                            .long("validation-timeout-seconds")
                            .value_parser(traversal::parse_depth),
                    );
                for option in ["boundary", "exclude", "refresh-source"] {
                    cmd = cmd.arg(Arg::new(option).long(option).action(ArgAction::Append));
                }
                cmd = cmd
                    .arg(
                        Arg::new("fallback")
                            .long("fallback")
                            .action(ArgAction::Append)
                            .value_parser(clap::value_parser!(BranchName)),
                    )
                    .arg(
                        Arg::new("pin")
                            .long("pin")
                            .action(ArgAction::Append)
                            .value_parser(clap::value_parser!(tf_plan::pins::PinRequest)),
                    )
                    .arg(
                        Arg::new("param")
                            .long("param")
                            .action(ArgAction::Append)
                            .value_parser(parameter),
                    );
            }
            cmd
        })
        .collect()
}
fn parameter(text: &str) -> Result<(String, Value), String> {
    if text.len() > 64 * 1024 {
        return Err("Parameter exceeds the input limit".into());
    }
    let (name, value) = text
        .split_once('=')
        .ok_or("Use name=JSON or dataset#name=JSON")?;
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("Invalid parameter selector".into());
    }
    let value = serde_json::from_str(value).map_err(|_| "Parameter value must be JSON")?;
    Ok((name.into(), value))
}
fn strings(args: &ArgMatches, key: &str) -> Vec<String> {
    args.get_many::<String>(key)
        .map(|v| v.cloned().collect())
        .unwrap_or_default()
}
pub(crate) struct Report {
    pub value: Value,
    pub human: String,
}
pub(crate) fn execute(
    name: &str,
    args: &ArgMatches,
    explicit: Option<&String>,
) -> Result<Report, Error> {
    let current = std::env::current_dir().map_err(failure)?;
    let workspace = tf_catalog::workspace::Workspace::load(&current, explicit.map(Path::new))
        .map_err(failure)?;
    let source = args.get_one::<String>("git-ref").cloned();
    let branch = git::output_branch(
        workspace.config(),
        git::inspect(&workspace).map_err(failure)?.as_ref(),
        args.get_one::<BranchName>("branch"),
        source.is_some(),
    )
    .map_err(failure)?;
    let owner = RuntimeOwner::acquire(
        workspace.root(),
        workspace.config().id(),
        CoordinatorMode::Temporary,
    )
    .map_err(failure)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(failure)?;
    let value = if matches!(name, "upstream" | "downstream") {
        let completion = runtime.block_on(graph_query::inspect_source(
            owner,
            graph_query::Request {
                python: args.get_one::<String>("python").cloned(),
                branch,
                start: args
                    .get_one::<String>("reference")
                    .cloned()
                    .ok_or_else(|| failure("Missing starting dataset"))?,
                direction: if name == "upstream" {
                    traversal::Direction::Upstream
                } else {
                    traversal::Direction::Downstream
                },
                depth: args.get_one::<u64>("depth").copied(),
            },
            source,
        ))?;
        graph_value(name, &completion.result?)?
    } else {
        let mut targets = strings(args, "targets");
        targets.extend(strings(args, "target"));
        targets.sort();
        targets.dedup();
        if name == "why" && targets.len() != 1 {
            return Err(failure("why requires exactly one distinct target"));
        }
        let request = build_plan::Request {
            python: args.get_one::<String>("python").cloned(),
            branch,
            mode: match args.get_one::<String>("mode").map(String::as_str) {
                Some("selected") => tf_plan::scope::Mode::Selected,
                Some("between") => tf_plan::scope::Mode::Between,
                _ => tf_plan::scope::Mode::Full,
            },
            targets,
            boundaries: strings(args, "boundary"),
            exclusions: strings(args, "exclude"),
            refresh_sources: strings(args, "refresh-source"),
            pins: args
                .get_many::<tf_plan::pins::PinRequest>("pin")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
            fallbacks: if args.get_flag("no-fallback") {
                Some(vec![])
            } else {
                args.get_many::<BranchName>("fallback")
                    .map(|v| v.cloned().collect())
            },
            force: args.get_flag("force"),
            parameters: json!({}),
        };
        let options = build_plan::Options {
            git_ref: source,
            require_current: args
                .get_one::<String>("boundary-policy")
                .is_some_and(|v| v == "require_current"),
            timeout_seconds: args.get_one::<u64>("timeout-seconds").copied(),
            validation_timeout_seconds: args.get_one::<u64>("validation-timeout-seconds").copied(),
            parameters: args
                .get_many::<(String, Value)>("param")
                .map(|v| v.cloned().collect())
                .unwrap_or_default(),
            explain: true,
        };
        if name == "why" {
            let completion =
                runtime.block_on(crate::why::inspect_selection(owner, request, options))?;
            let report = completion.result?;
            let plan = report
                .plan
                .as_ref()
                .map(|p| plan_value("plan", p))
                .transpose()?;
            json!({"kind":"why","workspace":report.workspace,"source":report.source,"source_digest":report.source_digest,"branch":report.branch,"target":report.target,"freshness_context":"selected_branch_heads","status":report.status,"plan":plan,"planning_error":report.planning_error,"producer_execution":false,"authoring_changed":false})
        } else {
            let completion =
                runtime.block_on(build_plan::prepare_with_options(owner, request, options))?;
            plan_value(name, &completion.result?)?
        }
    };
    let human = human(&value)?;
    Ok(Report { value, human })
}
fn graph_value(name: &str, traversal: &traversal::Traversal) -> Result<Value, Error> {
    let first = traversal.page(None, 100).map_err(failure)?;
    let mut nodes = vec![];
    let mut edges = vec![];
    let mut cursor = None;
    loop {
        let page = traversal.page(cursor.as_deref(), 100).map_err(failure)?;
        for n in page.nodes {
            nodes.push(json!({"identity":identity(&n.node.identity),"paths":n.node.paths.iter().map(|p|p.as_str()).collect::<Vec<_>>(),"depth":n.depth.to_string(),"external":n.node.external,"producer":n.node.producer}));
        }
        for e in page.edges {
            let branch = match e.declared_branch {
                BranchSelector::Omitted => json!({"kind":"omitted","name":null}),
                BranchSelector::Current => json!({"kind":"current","name":null}),
                BranchSelector::Named(n) => json!({"kind":"named","name":n.as_str()}),
            };
            edges.push(json!({"parent":identity(&e.parent),"consumer":identity(&e.consumer),"alias":e.alias,"role":e.role.name(),"declared_branch":branch,"stop_branch_fallback":e.stop_branch_fallback,"checks":e.checks}));
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    Ok(
        json!({"kind":"graph","direction":name,"workspace":first.context.workspace.to_string(),"source":first.context.source.to_string(),"source_digest":first.context.source_digest,"registry_fingerprint":first.context.registry_fingerprint,"certificate_fingerprint":first.context.certificate_fingerprint,"branch":first.context.branch.as_str(),"start":identity(&first.request.start),"depth":first.request.depth.map(|d|d.to_string()),"nodes":nodes,"edges":edges,"total_nodes":first.total_nodes.to_string(),"total_edges":first.total_edges.to_string(),"omitted_nodes":first.omitted_nodes.to_string(),"omitted_edges":first.omitted_edges.to_string(),"scope_complete":first.scope_complete,"delivery_complete":true,"external_expanded":false}),
    )
}
fn plan_value(name: &str, plan: &tf_store::planning::DraftPlan) -> Result<Value, Error> {
    let workspace = plan.workspace.parse().map_err(failure)?;
    let before = RegistrySnapshot::parse(workspace, &plan.registry).map_err(failure)?;
    let registry = RegistrySnapshot::parse(workspace, &plan.replacement).map_err(failure)?;
    let path = |id: &str| {
        registry
            .resolve_output(&format!("dataset:{id}"))
            .map(|d| d.path().to_string())
            .map_err(failure)
    };
    let pending = registry
        .datasets()
        .filter(|d| {
            before
                .resolve(&format!("dataset:{}", d.key().dataset_id()))
                .is_err()
        })
        .map(|d| json!({"dataset":d.key().dataset_id().to_string(),"path":d.path().as_str()}))
        .collect::<Vec<_>>();
    let writes=plan.writes.iter().map(|w|Ok(json!({"dataset":w.dataset,"path":path(&w.dataset)?,"job":w.job,"expected_generation":w.generation.to_string(),"bindings":w.bindings,"parameters":plan.context["parameters"][&w.dataset],"resources":plan.context["resources"][&w.dataset]}))).collect::<Result<Vec<_>,Error>>()?;
    let reads=plan.reads.iter().map(|r|Ok(json!({"consumer":r.consumer,"consumer_path":path(&r.consumer)?,"alias":r.alias,"dataset":r.dataset,"path":path(&r.dataset)?,"version":r.version,"artifact":r.artifact,"starting_branch":r.provenance["starting_branch"],"resolved_branch":r.provenance["resolved_branch"],"resolution":r.provenance["resolution"]["kind"],"currentness":"unknown"}))).collect::<Result<Vec<_>,Error>>()?;
    let warnings = if reads.is_empty() {
        vec![]
    } else {
        vec![
            "Read boundaries are byte-verified; currentness is unknown. Fallback and exact pins retain their own recorded context.",
        ]
    };
    Ok(
        json!({"kind":name,"workspace":plan.workspace,"source":plan.source,"source_digest":plan.source_digest,"branch":plan.output.name,"source_selector":plan.source_evidence["selector"],"source_commit":plan.source_evidence["git"]["commit"],"plan_id":plan.id,"digest":plan.digest().map_err(failure)?.hex(),"created_us":plan.created_us.to_string(),"expires_us":plan.expires_us.to_string(),"mode":plan.context["mode"],"force":plan.context["force"],"targets":plan.context["targets"],"boundaries":plan.context["boundaries"],"exclusions":plan.context["exclusions"],"refresh_sources":plan.context["refresh_sources"],"fallback_override":plan.context["fallback_override"],"boundary_policy":plan.context["boundary_policy"],"branch_would_be_created":plan.output.branch.is_none(),"pending_registrations":pending,"writes":writes,"reads":reads,"freshness":plan.context["freshness"].as_array().cloned().unwrap_or_default(),"freshness_context":"selected_branch_heads","warnings":warnings,"producer_execution":false,"authoring_changed":false}),
    )
}
fn human(v: &Value) -> Result<String, Error> {
    let redactor = Redactor::default();
    let safe = |s: &str| {
        redactor
            .text(s)
            .map(|v| v.as_str().to_owned())
            .map_err(failure)
    };
    let mut out = format!(
        "Source {} on data branch {}\n",
        v["source"].as_str().unwrap_or("unknown"),
        safe(v["branch"].as_str().unwrap_or("unknown"))?
    );
    if v["kind"] == "why" {
        out.push_str(&format!(
            "Why {} (freshness of selected-branch heads):\n",
            safe(v["target"].as_str().unwrap_or("unknown"))?
        ));
        let status = &v["status"];
        if status.is_null() {
            out.push_str("Foreign provider freshness is unknown.\n");
        } else {
            out.push_str(&format!("Materialization: {}; data: {}; logic: {}; ancestors: {}; latest attempt: {}; output quality: {}\n",status["materialization"].as_str().unwrap_or("Unknown"),status["direct_data"].as_str().unwrap_or("Unknown"),status["direct_logic"].as_str().unwrap_or("Unknown"),status["inherited"].as_str().unwrap_or("Unknown"),status["latest_attempt"].as_str().unwrap_or("none"),status["output_quality"].as_str().unwrap_or("Unavailable")));
            if let Some(reasons) = status["reasons"].as_array() {
                for r in reasons {
                    out.push_str(&format!(
                        "  {}\n",
                        safe(r["message"].as_str().unwrap_or(""))?
                    ));
                }
            }
        }
        if let Some(error) = v["planning_error"].as_str() {
            out.push_str(&format!("Selection cannot be planned: {}\n", safe(error)?));
        }
        if !v["plan"].is_null() {
            out.push_str(&human(&v["plan"])?);
        }
    } else if v["kind"] == "graph" {
        out.push_str(&format!(
            "{}: {} unique nodes, {} edges.\n",
            v["direction"].as_str().unwrap_or("Graph"),
            v["total_nodes"].as_str().unwrap_or("0"),
            v["total_edges"].as_str().unwrap_or("0")
        ));
        if let Some(nodes) = v["nodes"].as_array() {
            for n in nodes {
                out.push_str(&format!(
                    "  depth {}: {}{}\n",
                    n["depth"].as_str().unwrap_or("0"),
                    safe(n["paths"][0].as_str().unwrap_or("unknown"))?,
                    if n["external"] == true {
                        " [foreign read boundary]"
                    } else {
                        ""
                    }
                ));
            }
        }
        if let Some(edges) = v["edges"].as_array() {
            for e in edges {
                out.push_str(&format!(
                    "  {} -> {} #{} ({})\n",
                    safe(e["parent"].as_str().unwrap_or(""))?,
                    safe(e["consumer"].as_str().unwrap_or(""))?,
                    safe(e["alias"].as_str().unwrap_or(""))?,
                    e["role"].as_str().unwrap_or("")
                ));
            }
        }
        out.push_str(&format!("All requested pages delivered; depth omitted {} nodes and {} edges. Foreign provenance was not expanded.\n",v["omitted_nodes"].as_str().unwrap_or("0"),v["omitted_edges"].as_str().unwrap_or("0")));
    } else {
        out.push_str(&format!(
            "Draft {} ({}); expires at {} UTC microseconds.\nMode: {}; force: {}.\n",
            v["plan_id"].as_str().unwrap_or(""),
            v["digest"].as_str().unwrap_or(""),
            v["expires_us"].as_str().unwrap_or(""),
            v["mode"].as_str().unwrap_or(""),
            v["force"]
        ));
        if v["branch_would_be_created"] == true {
            out.push_str("The output branch would be created on acceptance.\n");
        }
        for (field, label) in [
            ("pending_registrations", "Pending registration"),
            ("writes", "Prospective write"),
            ("reads", "Read boundary"),
        ] {
            if let Some(entries) = v[field].as_array() {
                for e in entries {
                    out.push_str(&format!(
                        "  {label}: {}\n",
                        safe(e["path"].as_str().unwrap_or("unknown"))?
                    ));
                    if let Some(bindings) = e["bindings"].as_array() {
                        for b in bindings {
                            out.push_str(&format!(
                                "    #{}: {} parent {}\n",
                                safe(b["alias"].as_str().unwrap_or(""))?,
                                if b["kind"] == "planned" {
                                    "symbolic in-build"
                                } else {
                                    "read-boundary"
                                },
                                b["dataset"].as_str().unwrap_or("")
                            ));
                        }
                    }
                }
            }
        }
        if let Some(statuses) = v["freshness"].as_array() {
            for s in statuses {
                out.push_str(&format!(
                    "  {}: data {}, logic {}, ancestors {}, latest attempt {}, output checks {}\n",
                    safe(s["path"].as_str().unwrap_or("unknown"))?,
                    s["direct_data"].as_str().unwrap_or("Unknown"),
                    s["direct_logic"].as_str().unwrap_or("Unknown"),
                    s["inherited"].as_str().unwrap_or("Unknown"),
                    s["latest_attempt"].as_str().unwrap_or("none"),
                    s["output_quality"].as_str().unwrap_or("Unavailable")
                ));
                if let Some(reasons) = s["reasons"].as_array() {
                    for r in reasons {
                        out.push_str(&format!(
                            "    {}\n",
                            safe(r["message"].as_str().unwrap_or(""))?
                        ));
                    }
                }
            }
        }
        if let Some(warnings) = v["warnings"].as_array() {
            for w in warnings {
                out.push_str(&format!("Warning: {}\n", safe(w.as_str().unwrap_or(""))?));
            }
        }
        out.push_str(
            "No producers ran. No dataset versions, heads or authoring registrations changed.\n",
        );
    }
    Ok(out)
}
