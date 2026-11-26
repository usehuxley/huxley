//! The walk (ADR-021): a pass over the document against the journal that
//! records steps without side effects and control-flow decisions at once,
//! and schedules jobs for the rest. Nothing here touches the database;
//! [`crate::run`] loads the journal before and persists it after.

use std::collections::BTreeMap;

use huxley_expr::{Engine as Expr, ExprError, Scope};
use huxley_hwf::{
    ForeachStep, IfStep, Join, ParallelStep, Step, SwitchStep, TryStep, WaitForCallbackStep,
    WaitStep, Workflow,
};
use serde_json::{Map, Value as Json, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::journal::{Journal, Record, StepStatus, step_key};

/// Most items a `foreach` may iterate.
pub const MAX_FOREACH_ITEMS: usize = 10_000;

/// What a run's expressions can see besides `steps`.
#[derive(Debug, Clone, Default)]
pub struct RunContext {
    pub run_id: Uuid,
    pub org_id: Uuid,
    pub inputs: Json,
    /// Inputs that are secret (`writeOnly`): reading them taints.
    pub secret_inputs: std::collections::BTreeSet<String>,
    pub trigger: Json,
    /// Run-scoped variables (`run.vars`), updated by `set_var`.
    pub vars: Map<String, Json>,
    /// Workflow key/version, start time…: merged into `run`.
    pub meta: Map<String, Json>,
    pub actor: Json,
    /// Organization variables (`vars`).
    pub org_vars: Json,
    pub secrets: BTreeMap<String, String>,
    /// Where callback URLs point (`auth.public_url`), if set.
    pub public_url: Option<String>,
    /// The run's recorded `lookup()` answers, and the ones it still needs.
    pub lookups: std::sync::Arc<huxley_expr::Lookups>,
}

/// How a block or step stands after this pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    /// Finished, with the output of its last step and whether it is secret.
    Done(Json, bool),
    /// Waiting on a scheduled step.
    Pending,
    Failed(Json),
    /// A `return` ended the run with these outputs.
    Returned(Json),
}

/// The result of a whole pass.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pending,
    Succeeded(Json),
    Failed(Json),
}

/// A step job to queue: now, or at a time (`wait`).
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub step_key: String,
    pub run_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, Default)]
struct Frame {
    indices: Vec<usize>,
    locals: Map<String, Json>,
}

impl Frame {
    fn with(&self, name: &str, value: Json) -> Self {
        let mut next = self.clone();
        next.locals.insert(name.to_owned(), value);
        next
    }
}

pub fn error(kind: &str, message: impl Into<String>) -> Json {
    json!({ "kind": kind, "message": message.into() })
}

fn expr_error(e: &ExprError) -> Json {
    error("expression", e.to_string())
}

/// The kind of the error meaning "waiting for a `lookup()` answer": not a
/// failure. The step stays unrecorded; the engine fetches the answer and
/// walks again.
const LOOKUP_PENDING: &str = "lookup_pending";

fn is_lookup_pending(e: &Json) -> bool {
    e["kind"] == LOOKUP_PENDING
}

/// A `script` step's input, before expressions.
fn script_input(s: &huxley_hwf::ScriptStep) -> Json {
    let mut value = json!({ "script": s.script, "with": s.inputs });
    if !s.env.is_empty() {
        value["env"] = json!(s.env);
    }
    if !s.connections.is_empty() {
        value["connections"] = json!(s.connections);
    }
    value
}

/// An `approval` step's input: it expires `expires` after `now`.
fn approval_input(s: &huxley_hwf::ApprovalStep, now: OffsetDateTime) -> Json {
    // Validation checked `expires`; fall back to the default.
    let expires = huxley_hwf::duration::parse(s.expires.as_deref().unwrap_or("7d"))
        .or_else(|| huxley_hwf::duration::parse("7d"))
        .unwrap_or_default();
    json!({
        "title": s.title,
        "message": s.message,
        "approvers": s.approvers,
        "expires_at": (now + expires).format(&Rfc3339).unwrap_or_default(),
    })
}

/// An `integration.action` step's input, before expressions.
fn action_input(s: &huxley_hwf::IntegrationActionStep) -> Json {
    let mut value = json!({ "action": s.action, "connection": s.connection, "inputs": s.inputs });
    if s.all_pages {
        value["all_pages"] = json!(true);
        value["max_pages"] = json!(s.max_pages);
    }
    value
}

const fn is_container(step: &Step) -> bool {
    matches!(
        step,
        Step::If(_) | Step::Switch(_) | Step::Foreach(_) | Step::Parallel(_) | Step::Try(_)
    )
}

pub struct Walker<'a> {
    expr: &'a Expr,
    ctx: &'a mut RunContext,
    journal: &'a mut Journal,
    now: OffsetDateTime,
    pub schedules: Vec<Schedule>,
}

impl<'a> Walker<'a> {
    pub fn new(
        expr: &'a Expr,
        ctx: &'a mut RunContext,
        journal: &'a mut Journal,
        now: OffsetDateTime,
    ) -> Self {
        Self {
            expr,
            ctx,
            journal,
            now,
            schedules: Vec::new(),
        }
    }

    /// Walks the whole run.
    pub fn run(&mut self, workflow: &Workflow) -> Outcome {
        let root = Frame::default();
        match self.block(&workflow.spec.steps, &root) {
            Flow::Pending => Outcome::Pending,
            Flow::Returned(outputs) => Outcome::Succeeded(outputs),
            Flow::Done(..) => match &workflow.spec.outputs {
                None => Outcome::Succeeded(json!({})),
                Some(outputs) => {
                    let value = Json::Object(outputs.clone().into_iter().collect());
                    match self.eval(&value, &root) {
                        Ok((value, false)) => Outcome::Succeeded(value),
                        Ok((_, true)) => Outcome::Failed(error(
                            "outputs",
                            "the workflow's outputs include a secret",
                        )),
                        Err(e) if is_lookup_pending(&e) => Outcome::Pending,
                        Err(e) => Outcome::Failed(e),
                    }
                }
            },
            Flow::Failed(e) if workflow.spec.on_error.is_empty() => Outcome::Failed(e),
            Flow::Failed(e) => {
                let frame = root.with("error", e.clone());
                match self.block(&workflow.spec.on_error, &frame) {
                    Flow::Pending => Outcome::Pending,
                    _ => Outcome::Failed(e),
                }
            }
        }
    }

    fn scope(&self, frame: &Frame) -> Scope {
        let mut run = self.ctx.meta.clone();
        run.insert("id".into(), json!(self.ctx.run_id));
        run.insert("vars".into(), Json::Object(self.ctx.vars.clone()));
        let mut scope = Scope::new()
            .with_partly_secret("inputs", &self.ctx.inputs, &self.ctx.secret_inputs)
            .with("trigger", &self.ctx.trigger)
            .with("steps", &self.journal.visible(&frame.indices))
            .with("vars", &self.ctx.org_vars)
            .with("run", &Json::Object(run))
            .with("org", &json!({ "id": self.ctx.org_id }))
            .with("actor", &self.ctx.actor)
            .with("env", &json!({}))
            .with("connections", &json!({}))
            .with_secrets(self.ctx.secrets.clone())
            .with_lookups(self.ctx.lookups.clone());
        for (name, value) in &frame.locals {
            scope.insert(name, value);
        }
        scope
    }

    /// Evaluates a value. Tainted if it read a secret or the output of a step
    /// whose output is secret.
    fn eval(&self, value: &Json, frame: &Frame) -> Result<(Json, bool), Json> {
        let waiting = self.ctx.lookups.missing_count();
        let evaluated = self
            .expr
            .eval(value, &self.scope(frame))
            .map_err(|e| self.expr_failure(&e, waiting))?;
        let tainted = evaluated.tainted || self.reads_secret_output(value, frame);
        Ok((evaluated.value, tainted))
    }

    /// A condition, with Jinja truthiness.
    fn condition(&self, text: &str, frame: &Frame) -> Result<bool, Json> {
        let waiting = self.ctx.lookups.missing_count();
        self.expr
            .eval_condition(text, &self.scope(frame))
            .map_err(|e| self.expr_failure(&e, waiting))
    }

    /// An evaluation's error, or [`LOOKUP_PENDING`] if it stopped because it
    /// asked for a lookup the run hasn't recorded yet.
    fn expr_failure(&self, e: &ExprError, waiting_before: usize) -> Json {
        if self.ctx.lookups.missing_count() > waiting_before {
            error(LOOKUP_PENDING, "waiting for a lookup")
        } else {
            expr_error(e)
        }
    }

    fn reads_secret_output(&self, value: &Json, frame: &Frame) -> bool {
        let secret_ids: Vec<&str> = self
            .journal
            .records()
            .filter(|r| r.output_tainted && frame.indices.starts_with(&r.indices))
            .map(|r| r.step_id.as_str())
            .collect();
        if secret_ids.is_empty() {
            return false;
        }
        let mut strings = Vec::new();
        collect_strings(value, &mut strings);
        strings.iter().any(|s| {
            self.expr.references(s).is_ok_and(|refs| {
                refs.iter().any(|r| {
                    r.strip_prefix("steps.")
                        .and_then(|rest| rest.split('.').next())
                        .is_some_and(|id| secret_ids.contains(&id))
                })
            })
        })
    }

    fn block(&mut self, steps: &[Step], frame: &Frame) -> Flow {
        let mut last = (Json::Null, false);
        for step in steps {
            match self.step(step, frame) {
                Flow::Done(output, tainted) => last = (output, tainted),
                other => return other,
            }
        }
        Flow::Done(last.0, last.1)
    }

    /// Records a new step that settled immediately.
    fn settle_new(
        &mut self,
        step: &Step,
        frame: &Frame,
        result: Result<(Json, bool), Json>,
    ) -> Flow {
        if let Err(e) = &result
            && is_lookup_pending(e)
        {
            return Flow::Pending;
        }
        let common = step.common();
        let mut record = Record::new(
            common.id,
            step.type_name(),
            &frame.indices,
            StepStatus::Succeeded,
        );
        let flow = match result {
            Ok((output, tainted)) => {
                record.output = Some(output.clone());
                record.output_tainted = tainted;
                Flow::Done(output, tainted)
            }
            Err(e) => {
                record.status = StepStatus::Failed;
                record.error = Some(e.clone());
                if common.continue_on_error {
                    Flow::Done(e, false)
                } else {
                    Flow::Failed(e)
                }
            }
        };
        self.journal.put(record);
        flow
    }

    // Lists every step type: grows by a line with each one.
    #[allow(clippy::too_many_lines)]
    fn step(&mut self, step: &Step, frame: &Frame) -> Flow {
        let common = step.common();
        let key = step_key(common.id, &frame.indices);
        if let Some(record) = self.journal.get(&key) {
            match record.status {
                StepStatus::Succeeded | StepStatus::Skipped => {
                    return Flow::Done(
                        record.output.clone().unwrap_or(Json::Null),
                        record.output_tainted,
                    );
                }
                StepStatus::Failed if common.continue_on_error => {
                    return Flow::Done(record.error.clone().unwrap_or(Json::Null), false);
                }
                StepStatus::Failed | StepStatus::Cancelled => {
                    return Flow::Failed(
                        record
                            .error
                            .clone()
                            .unwrap_or_else(|| error("cancelled", "the step was cancelled")),
                    );
                }
                StepStatus::Running | StepStatus::Waiting if !is_container(step) => {
                    return Flow::Pending;
                }
                // A container in progress: carry on with its journaled decision.
                StepStatus::Running | StepStatus::Waiting => {}
            }
        } else if let Some(condition) = common.when {
            match self.condition(condition, frame) {
                Ok(true) => {}
                Ok(false) => {
                    let mut record = Record::new(
                        common.id,
                        step.type_name(),
                        &frame.indices,
                        StepStatus::Skipped,
                    );
                    record.output = Some(Json::Null);
                    self.journal.put(record);
                    return Flow::Done(Json::Null, false);
                }
                Err(e) => return self.settle_new(step, frame, Err(e)),
            }
        }

        match step {
            Step::Transform(s) => {
                let result = self.eval(&s.output, frame);
                self.settle_new(step, frame, result)
            }
            Step::SetVar(_) | Step::Return(_) | Step::Fail(_) => self.instant(step, frame),
            Step::Wait(s) => self.wait(step, s, frame),
            Step::HttpRequest(_)
            | Step::Email(_)
            | Step::Notify(_)
            | Step::DatasetQuery(_)
            | Step::DatasetUpsert(_)
            | Step::ContactFind(_)
            | Step::ContactUpsert(_)
            | Step::CompanyGet(_)
            | Step::CompanyUpdate(_)
            | Step::TicketFromEmail(_)
            | Step::MailFetch(_)
            | Step::TicketEmbed(_)
            | Step::KbEmbed(_)
            | Step::TimesheetCheck(_)
            | Step::CsatSurvey(_)
            | Step::NpsSend(_)
            | Step::HealthCompute(_)
            | Step::SentimentRecord(_)
            | Step::TriageOptions(_)
            | Step::TriageApply(_)
            | Step::DispatchRecommend(_)
            | Step::WorkforceInsights(_)
            | Step::TicketInsight(_)
            | Step::KbDraft(_)
            | Step::TimeSuggest(_)
            | Step::ReportSend(_)
            | Step::ChangeDecide(_)
            | Step::ScopeDecide(_)
            | Step::AssetSync(_)
            | Step::AssetWarranty(_)
            | Step::TicketEmailReply(_)
            | Step::TicketCreate(_)
            | Step::TicketGet(_)
            | Step::TicketFind(_)
            | Step::TicketUpdate(_)
            | Step::TicketNote(_)
            | Step::TicketDispatch(_)
            | Step::AiPrompt(_)
            | Step::AiClassify(_)
            | Step::AiExtract(_) => self.external(step, &with_block(step), frame),
            Step::Subworkflow(s) => {
                let value = json!({ "workflow": s.workflow, "inputs": s.inputs, "wait": s.wait });
                self.external(step, &value, frame)
            }
            Step::WaitForCallback(s) => self.callback_step(step, s, frame),
            Step::Script(s) => self.external(step, &script_input(s), frame),
            Step::Approval(s) => {
                let value = approval_input(s, self.now);
                self.external(step, &value, frame)
            }
            Step::IntegrationAction(s) => self.external(step, &action_input(s), frame),
            Step::If(s) => self.if_step(step, s, frame),
            Step::Switch(s) => self.switch_step(step, s, frame),
            Step::Foreach(s) => self.foreach_step(step, s, frame),
            Step::Parallel(s) => self.parallel_step(step, s, frame),
            Step::Try(s) => self.try_step(step, s, frame),
        }
    }

    /// `set_var`, `return` and `fail`: evaluated and settled at once.
    fn instant(&mut self, step: &Step, frame: &Frame) -> Flow {
        match step {
            Step::SetVar(s) => {
                let value = Json::Object(s.vars.clone().into_iter().collect());
                let result = match self.eval(&value, frame) {
                    Ok((_, true)) => Err(error(
                        "secret",
                        "set_var can't store secrets in run variables; use a transform step",
                    )),
                    Ok((Json::Object(vars), false)) => {
                        self.ctx.vars.extend(vars.clone());
                        Ok((Json::Object(vars), false))
                    }
                    Ok(_) => Err(error("expression", "set_var needs an object")),
                    Err(e) => Err(e),
                };
                self.settle_new(step, frame, result)
            }
            Step::Return(s) => {
                let value =
                    Json::Object(s.outputs.clone().unwrap_or_default().into_iter().collect());
                match self.eval(&value, frame) {
                    Ok((outputs, false)) => {
                        self.settle_new(step, frame, Ok((outputs.clone(), false)));
                        Flow::Returned(outputs)
                    }
                    Ok((_, true)) => self.settle_new(
                        step,
                        frame,
                        Err(error("secret", "return outputs can't include secrets")),
                    ),
                    Err(e) => self.settle_new(step, frame, Err(e)),
                }
            }
            Step::Fail(s) => {
                let failure = match self.eval(&json!(s.message), frame) {
                    Ok((message, _)) => {
                        let message = message
                            .as_str()
                            .map_or_else(|| message.to_string(), str::to_owned);
                        let mut e = error("fail", message);
                        if let Some(code) = &s.code {
                            e["code"] = json!(code);
                        }
                        e
                    }
                    Err(e) => e,
                };
                self.settle_new(step, frame, Err(failure))
            }
            _ => unreachable!("only instant steps"),
        }
    }

    /// Evaluates a step's input and schedules it.
    fn external(&mut self, step: &Step, value: &Json, frame: &Frame) -> Flow {
        match self.eval(value, frame) {
            Ok((input, tainted)) => {
                let mut record = Record::new(
                    step.id(),
                    step.type_name(),
                    &frame.indices,
                    StepStatus::Running,
                );
                record.input = Some(input);
                record.tainted = tainted;
                self.schedules.push(Schedule {
                    step_key: record.step_key.clone(),
                    run_at: None,
                });
                self.journal.put(record);
                Flow::Pending
            }
            Err(e) => self.settle_new(step, frame, Err(e)),
        }
    }

    /// `wait_for_callback`: makes the step's single-use URL, evaluates
    /// `send` with `callback.url` in scope, and schedules the step job that
    /// records it, sends it, and parks until it's called or expires. The
    /// token is a credential, so the input is sealed.
    fn callback_step(&mut self, step: &Step, s: &WaitForCallbackStep, frame: &Frame) -> Flow {
        let Some(base) = self.ctx.public_url.clone() else {
            return self.settle_new(
                step,
                frame,
                Err(error(
                    "definition",
                    "callbacks need an address: set auth.public_url",
                )),
            );
        };
        let Some(expires) = huxley_hwf::duration::parse(s.expires.as_deref().unwrap_or("24h"))
        else {
            return self.settle_new(
                step,
                frame,
                Err(error("definition", "expires isn't a duration")),
            );
        };
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the OS random source works");
        let token =
            base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes);
        let url = format!("{}/callbacks/{token}", base.trim_end_matches('/'));
        let send = match &s.send {
            Some(send) => {
                let inner = frame.with("callback", json!({ "url": url }));
                let value = serde_json::to_value(send).unwrap_or(Json::Null);
                match self.eval(&value, &inner) {
                    Ok((value, _)) => value,
                    Err(e) => return self.settle_new(step, frame, Err(e)),
                }
            }
            None => Json::Null,
        };
        let mut record = Record::new(
            step.id(),
            step.type_name(),
            &frame.indices,
            StepStatus::Running,
        );
        record.input = Some(json!({
            "token": token,
            "url": url,
            "expires_at": (self.now + expires).format(&Rfc3339).unwrap_or_default(),
            "send": send,
        }));
        record.tainted = true;
        self.schedules.push(Schedule {
            step_key: record.step_key.clone(),
            run_at: None,
        });
        self.journal.put(record);
        Flow::Pending
    }

    fn wait(&mut self, step: &Step, s: &WaitStep, frame: &Frame) -> Flow {
        let until = match (&s.duration, &s.until) {
            (Some(d), _) => huxley_hwf::duration::parse(d)
                .map(|d| self.now + d)
                .ok_or_else(|| error("expression", format!("{d:?} is not a duration"))),
            (None, Some(until)) => self.eval(&json!(until), frame).and_then(|(v, _)| {
                v.as_str()
                    .and_then(|t| OffsetDateTime::parse(t, &Rfc3339).ok())
                    .ok_or_else(|| error("expression", "wait.until must be an RFC 3339 timestamp"))
            }),
            (None, None) => Err(error("definition", "wait needs duration or until")),
        };
        match until {
            Ok(until) => {
                let until = until.max(self.now);
                let mut record = Record::new(
                    step.id(),
                    step.type_name(),
                    &frame.indices,
                    StepStatus::Waiting,
                );
                record.input = Some(json!({ "until": until.format(&Rfc3339).unwrap_or_default() }));
                self.schedules.push(Schedule {
                    step_key: record.step_key.clone(),
                    run_at: Some(until),
                });
                self.journal.put(record);
                Flow::Pending
            }
            Err(e) => self.settle_new(step, frame, Err(e)),
        }
    }

    /// The journaled decision of a container in progress, or `None` if it
    /// hasn't started (the caller then decides and calls [`Self::begin`]).
    fn decision(&self, step: &Step, frame: &Frame) -> Option<Json> {
        self.journal
            .get(&step_key(step.id(), &frame.indices))
            .and_then(|r| r.input.clone())
    }

    fn begin(&mut self, step: &Step, frame: &Frame, decision: Json) {
        let mut record = Record::new(
            step.id(),
            step.type_name(),
            &frame.indices,
            StepStatus::Running,
        );
        record.input = Some(decision);
        self.journal.put(record);
    }

    /// Records how a container's children came out.
    fn finish(&mut self, step: &Step, frame: &Frame, flow: Flow) -> Flow {
        let key = step_key(step.id(), &frame.indices);
        let continue_on_error = step.common().continue_on_error;
        let Some(record) = self.journal.get_mut(&key) else {
            return flow;
        };
        match flow {
            Flow::Done(output, tainted) => {
                record.status = StepStatus::Succeeded;
                record.output = Some(output.clone());
                record.output_tainted = tainted;
                Flow::Done(output, tainted)
            }
            Flow::Failed(e) => {
                record.status = StepStatus::Failed;
                record.error = Some(e.clone());
                if continue_on_error {
                    Flow::Done(e, false)
                } else {
                    Flow::Failed(e)
                }
            }
            Flow::Returned(outputs) => {
                record.status = StepStatus::Succeeded;
                record.output = Some(Json::Null);
                Flow::Returned(outputs)
            }
            Flow::Pending => Flow::Pending,
        }
    }

    fn if_step(&mut self, step: &Step, s: &IfStep, frame: &Frame) -> Flow {
        let branch = match self.decision(step, frame) {
            Some(d) => d["branch"].as_str().unwrap_or("else").to_owned(),
            None => match self.condition(&s.condition, frame) {
                Ok(yes) => {
                    let branch = if yes { "then" } else { "else" };
                    self.begin(step, frame, json!({ "branch": branch }));
                    branch.to_owned()
                }
                Err(e) => return self.settle_new(step, frame, Err(e)),
            },
        };
        let block = if branch == "then" {
            &s.then
        } else {
            &s.otherwise
        };
        let flow = self.block(block, frame);
        self.finish(step, frame, flow)
    }

    fn switch_step(&mut self, step: &Step, s: &SwitchStep, frame: &Frame) -> Flow {
        let case = if let Some(d) = self.decision(step, frame) {
            d["case"].as_u64().and_then(|c| usize::try_from(c).ok())
        } else {
            match self.choose_case(s, frame) {
                Ok((chosen, value)) => {
                    self.begin(step, frame, json!({ "case": chosen, "value": value }));
                    chosen
                }
                Err(e) => return self.settle_new(step, frame, Err(e)),
            }
        };
        let block = case
            .and_then(|i| s.cases.get(i))
            .map_or(&s.default, |c| &c.steps);
        let flow = self.block(block, frame);
        self.finish(step, frame, flow)
    }

    /// The first case whose `when` equals the switch's value.
    fn choose_case(&self, s: &SwitchStep, frame: &Frame) -> Result<(Option<usize>, Json), Json> {
        let (value, _) = self.eval(&json!(s.value), frame)?;
        for (i, case) in s.cases.iter().enumerate() {
            if self.eval(&case.when, frame)?.0 == value {
                return Ok((Some(i), value));
            }
        }
        Ok((None, value))
    }

    fn foreach_step(&mut self, step: &Step, s: &ForeachStep, frame: &Frame) -> Flow {
        let items = match self.decision(step, frame) {
            Some(d) => d["items"].as_array().cloned().unwrap_or_default(),
            None => match self.eval(&json!(s.items), frame) {
                Ok((Json::Array(items), _)) if items.len() <= MAX_FOREACH_ITEMS => {
                    self.begin(step, frame, json!({ "items": items }));
                    items
                }
                Ok((Json::Array(_), _)) => {
                    return self.settle_new(
                        step,
                        frame,
                        Err(error(
                            "limit",
                            format!("foreach iterates at most {MAX_FOREACH_ITEMS} items"),
                        )),
                    );
                }
                Ok((other, _)) => {
                    return self.settle_new(
                        step,
                        frame,
                        Err(error(
                            "expression",
                            format!("foreach items must be a list, not {other}"),
                        )),
                    );
                }
                Err(e) => return self.settle_new(step, frame, Err(e)),
            },
        };
        let concurrency = s.concurrency.unwrap_or(1).max(1) as usize;
        let name = s.item_name.as_deref().unwrap_or("item");
        let length = items.len();
        let mut outputs = vec![Json::Null; length];
        let (mut active, mut waiting, mut failure, mut tainted) = (0, false, None, false);
        for (i, item) in items.into_iter().enumerate() {
            let mut indices = frame.indices.clone();
            indices.push(i);
            if !self.journal.iteration_started(&indices)
                && (active >= concurrency || failure.is_some())
            {
                waiting = true;
                continue;
            }
            let mut inner = frame.with(name, item).with(
                "loop",
                json!({
                    "index": i + 1, "index0": i, "first": i == 0, "last": i + 1 == length,
                    "length": length, "revindex": length - i, "revindex0": length - i - 1,
                }),
            );
            inner.indices = indices;
            match self.block(&s.steps, &inner) {
                Flow::Done(output, secret) => {
                    outputs[i] = output;
                    tainted |= secret;
                }
                Flow::Pending => active += 1,
                Flow::Failed(e) => failure = failure.or(Some(e)),
                Flow::Returned(o) => return self.finish(step, frame, Flow::Returned(o)),
            }
        }
        // A failure stops new iterations; the step fails once the running
        // ones have settled.
        let flow = match failure {
            _ if active > 0 => Flow::Pending,
            Some(e) => Flow::Failed(e),
            None if waiting => Flow::Pending,
            None => Flow::Done(Json::Array(outputs), tainted),
        };
        self.finish(step, frame, flow)
    }

    fn parallel_step(&mut self, step: &Step, s: &ParallelStep, frame: &Frame) -> Flow {
        if self.decision(step, frame).is_none() {
            self.begin(step, frame, json!({}));
        }
        let mut outputs = Map::new();
        let (mut pending, mut failure, mut tainted) = (false, None, false);
        for branch in &s.branches {
            match self.block(&branch.steps, frame) {
                Flow::Done(output, secret) => {
                    if s.join == Join::Any {
                        let won = json!({ branch.id.clone(): output });
                        return self.finish(step, frame, Flow::Done(won, secret));
                    }
                    outputs.insert(branch.id.clone(), output);
                    tainted |= secret;
                }
                Flow::Pending => pending = true,
                Flow::Failed(e) => failure = failure.or(Some(e)),
                Flow::Returned(o) => return self.finish(step, frame, Flow::Returned(o)),
            }
        }
        let flow = match (pending, failure) {
            (true, _) => Flow::Pending,
            (false, Some(e)) => Flow::Failed(e),
            (false, None) => Flow::Done(Json::Object(outputs), tainted),
        };
        self.finish(step, frame, flow)
    }

    fn try_step(&mut self, step: &Step, s: &TryStep, frame: &Frame) -> Flow {
        if self.decision(step, frame).is_none() {
            self.begin(step, frame, json!({}));
        }
        let outcome = match self.block(&s.steps, frame) {
            Flow::Failed(e) if !s.catch.is_empty() => self.block(&s.catch, &frame.with("error", e)),
            other => other,
        };
        let flow = match outcome {
            Flow::Pending => Flow::Pending,
            Flow::Returned(o) => Flow::Returned(o),
            settled if s.finally.is_empty() => settled,
            settled => match self.block(&s.finally, frame) {
                Flow::Done(..) => settled,
                other => other,
            },
        };
        self.finish(step, frame, flow)
    }
}

fn collect_strings<'v>(value: &'v Json, out: &mut Vec<&'v str>) {
    match value {
        Json::String(s) => out.push(s),
        Json::Array(items) => items.iter().for_each(|v| collect_strings(v, out)),
        Json::Object(map) => map.values().for_each(|v| collect_strings(v, out)),
        _ => {}
    }
}

/// A step's `with:` block, for steps that hand it straight to their executor.
fn with_block(step: &Step) -> Json {
    let value = match step {
        Step::HttpRequest(s) => serde_json::to_value(&s.request),
        Step::Email(s) => serde_json::to_value(&s.email),
        Step::Notify(s) => serde_json::to_value(&s.notification),
        Step::DatasetQuery(s) => serde_json::to_value(&s.query),
        Step::DatasetUpsert(s) => serde_json::to_value(&s.upsert),
        Step::ContactFind(s) => serde_json::to_value(&s.find),
        Step::ContactUpsert(s) => serde_json::to_value(&s.contact),
        Step::CompanyGet(s) => serde_json::to_value(&s.company),
        Step::CompanyUpdate(s) => serde_json::to_value(&s.company),
        Step::TicketFromEmail(s) => serde_json::to_value(&s.email),
        Step::MailFetch(s) => serde_json::to_value(&s.fetch),
        Step::TicketEmbed(s) => serde_json::to_value(&s.embed),
        Step::KbEmbed(s) => serde_json::to_value(&s.embed),
        Step::TimesheetCheck(s) => serde_json::to_value(&s.check),
        Step::CsatSurvey(s) => serde_json::to_value(&s.survey),
        Step::NpsSend(s) => serde_json::to_value(&s.send),
        Step::HealthCompute(s) => serde_json::to_value(&s.compute),
        Step::SentimentRecord(s) => serde_json::to_value(&s.record),
        Step::TriageOptions(s) => serde_json::to_value(&s.options),
        Step::TriageApply(s) => serde_json::to_value(&s.apply),
        Step::DispatchRecommend(s) => serde_json::to_value(&s.recommend),
        Step::WorkforceInsights(s) => serde_json::to_value(&s.insights),
        Step::TicketInsight(s) => serde_json::to_value(&s.insight),
        Step::KbDraft(s) => serde_json::to_value(&s.draft),
        Step::TimeSuggest(s) => serde_json::to_value(&s.suggest),
        Step::ReportSend(s) => serde_json::to_value(&s.report),
        Step::ChangeDecide(s) => serde_json::to_value(&s.decide),
        Step::ScopeDecide(s) => serde_json::to_value(&s.decide),
        Step::AssetSync(s) => serde_json::to_value(&s.sync),
        Step::AssetWarranty(s) => serde_json::to_value(&s.warranty),
        Step::TicketEmailReply(s) => serde_json::to_value(&s.reply),
        Step::TicketCreate(s) => serde_json::to_value(&s.ticket),
        Step::TicketGet(s) => serde_json::to_value(&s.ticket),
        Step::TicketFind(s) => serde_json::to_value(&s.ticket),
        Step::TicketUpdate(s) => serde_json::to_value(&s.ticket),
        Step::TicketNote(s) => serde_json::to_value(&s.ticket),
        Step::TicketDispatch(s) => serde_json::to_value(&s.ticket),
        Step::AiPrompt(s) => serde_json::to_value(&s.ai),
        Step::AiClassify(s) => serde_json::to_value(&s.ai),
        Step::AiExtract(s) => serde_json::to_value(&s.ai),
        _ => Ok(Json::Null),
    };
    value.unwrap_or(Json::Null)
}
