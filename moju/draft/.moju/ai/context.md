# Moju AI Context

This file is context for `.moju/ai/ai-task.md`. Follow the selected AI task, not a generic fix task.

## Project
/Users/zuowenjian/devspace/rust/wfusion/wp-reactor/moju/draft

## Active View
Structures

## Active Domain
Lang

## Selected Element
Struct `Lang.AggPlan`

## Model Summary
124 structs, 13 flows, 30 modules, 10 verify cases

## Diagnostics
- none

## Related Files
- /Users/zuowenjian/devspace/rust/wfusion/wp-reactor/moju/draft/domain/lang/domain.mju

## Source Snippets
### /Users/zuowenjian/devspace/rust/wfusion/wp-reactor/moju/draft/domain/lang/domain.mju

```mju
// ---------------------------------------------------------------------------
// lang domain — WFL DSL language concepts
// Crate: wf-lang
// ---------------------------------------------------------------------------

command CompileRequest {
  source: String
  schema_paths: List<String>
}

struct WindowSchema {
  name: String
  streams: List<String>
  time_field: String
  over: String
  fields: List<FieldDef>
}

struct FieldDef {
  name: String
  field_type: FieldType
}

state BaseType {
  Chars
  Digit
  Float
  Bool
  Time
  Ip
  Hex
}

state FieldType {
  Base
  Array
}

// -- WFL File ---------------------------------------------------------------

struct WflFile {
  uses: List<UseDecl>
  patterns: List<PatternDecl>
  rules: List<RuleDecl>
  tests: List<TestBlock>
}

struct UseDecl {
  path: String
}

struct PatternDecl {
  name: String
  params: List<String>
  body: String
}

// -- Rule -------------------------------------------------------------------

struct RuleDecl {
  name: String
  meta: MetaBlock
  events: EventsBlock
  match_clause: MatchClause
  each_clause: EachClause
  score: ScoreExpr
  joins: List<JoinClause>
  pipeline_stages: List<PipelineStage>
  entity: EntityClause
  yield_clause: YieldClause
  conv: ConvClause
  limits: LimitsBlock
}

struct MetaBlock {
  entries: List<MetaEntry>
}

struct MetaEntry {
  key: String
  value: String
}

struct PipelineStage {
  match_clause: MatchClause
  joins: List<JoinClause>
}

// -- Events -----------------------------------------------------------------

struct EventsBlock {
  events: List<EventDecl>
}

struct EventDecl {
  alias: String
  window: String
  filter: String
}

// -- Match ------------------------------------------------------------------

struct MatchClause {
  steps: List<MatchStep>
  close: CloseBlock
  key_map: List<KeyMapItem>
}

struct EachClause {
  alias: String
  expr: String
}

struct KeyMapItem {
  logical_name: String
  alias: String
  field: String
}

struct MatchStep {
  branches: List<StepBranch>
}

struct StepBranch {
  label: String
  source: String
  field: String
  guard: String
}

struct PipeChain {
  stages: List<String>
}

// -- Close ------------------------------------------------------------------

struct CloseBlock {
  triggers: List<CloseTrigger>
  mode: CloseMode
}

state CloseMode {
  Or
  And
}

state CloseTrigger {
  Timeout
  Flush
  Eos
}

state WindowMode {
  Sliding
  Fixed
  Session
}

// -- Expression -------------------------------------------------------------

struct FieldSelector {
  path: String
}

struct FieldRef {
  alias: String
  field: String
}

state CmpOp {
  Eq
  Ne
  Lt
  Gt
  Le
  Ge
}

state BinOp {
  Add
  Sub
  Mul
  Div
  And
  Or
}

state SystemVar {
  Score
}

state Expr {
  Number
  StringLit
  BoolVal
  SystemVarRef
  FieldRef
  BinOpExpr
  Neg
  FuncCall
  InList
  IfThenElse
}

// -- Clauses ----------------------------------------------------------------

struct ScoreExpr {
  expr: Expr
}

struct EntityClause {
  entity_type: String
  entity_id: String
}

struct YieldClause {
  target: String
  version: String
  fields: List<YieldField>
}

struct YieldField {
  name: String
  value: String
}

// -- Join -------------------------------------------------------------------

struct JoinClause {
  right_window: String
  mode: JoinMode
  conditions: List<JoinCondition>
}

state JoinMode {
  Snapshot
  Asof
}

struct JoinCondition {
  left: String
  right: String
}

// -- Conv -------------------------------------------------------------------

struct ConvClause {
  chains: List<ConvChain>
}

struct ConvChain {
  steps: List<ConvStep>
}

state ConvStep {
  Sort
  Top
  Dedup
  Where
}

struct SortKey {
  field: String
  descending: Bool
}

// -- Limits -----------------------------------------------------------------

struct LimitsBlock {
  items: List<LimitItem>
}

struct LimitItem {
  limit_type: String
  value: Int
  per: String
  action: String
}

// -- Contract / Test --------------------------------------------------------

struct TestBlock {
  name: String
  inputs: List<InputStmt>
  expects: List<ExpectStmt>
  options: Map
}

state InputStmt {
  Row
  Tick
}

struct FieldAssign {
  field: String
  value: String
}

struct ExpectStmt {
  hits: List<HitAssert>
  asserts: Map
}

struct HitAssert {
  score: Int
  origin: String
  entity_type: String
  entity_id: String
  field_match: Map
}

state EvalMode {
  Strict
  Lenient
}

state PermutationMode {
  Shuffle
}

// -- Compilation plans (compiled AST -> runtime) --------------------------

struct RulePlan {
  name: String
  binds: List<BindPlan>
  match_plan: MatchPlan
  each_plan: EachClause
  joins: List<JoinPlan>
  entity_plan: EntityPlan
  yield_plan: YieldPlan
  score_plan: ScorePlan
  conv_plan: ConvPlan
  limits_plan: LimitsPlan
}

struct BindPlan {
  alias: String
  window: String
  filter: String
}

struct MatchPlan {
  keys: List<String>
  key_map: List<KeyMapPlan>
  window_spec: WindowSpec
  event_steps: List<StepPlan>
  close_steps: List<StepPlan>
  close_mode: CloseMode
}

struct KeyMapPlan {
  logical_name: String
  source_alias: String
  source_field: String
}

state WindowSpec {
  Sliding
  Fixed
  Session
}

struct StepPlan {
  branches: List<BranchPlan>
}

struct BranchPlan {
  label: String
  source: String
  field: String
  guard: String
  agg: AggPlan
}

struct AggPlan {
  transforms: List<String>
  measure: String
  cmp: CmpOp
  threshold: Int
}

struct JoinPlan {
  right_window: String
  mode: JoinMode
  conditions: List<JoinCondPlan>
}

struct JoinCondPlan {
  left_field: String
  right_field: String
}

struct LimitsPlan {
  max_memory_bytes: Int
  max_instances: Int
  max_throttle: RateSpec
  on_exceed: ExceedAction
}

state ExceedAction {
  Throttle
  DropOldest
  FailRule
}

struct RateSpec {
  count: Int
  per: String
}

struct EntityPlan {
  entity_type: String
  entity_id_expr: String
}

struct ScorePlan {
  expr: Expr
}

struct YieldPlan {
  target: String
  version: String
  fields: List<YieldField>
}

struct ConvPlan {
  chains: List<ConvChainPlan>
}

struct ConvChainPlan {
  ops: List<ConvOpPlan>
}

state ConvOpPlan {
  Sort
  Top
  Dedup
  WhereExpr
}

struct SortKeyPlan {
  expr: String
  descending: Bool
}

// -- Error types -------------------------------------------------------------

struct CheckError {
  severity: Severity
  message: String
  position: String
}

state Severity {
  Error
  Warning
  Note
}

state LangReason {
  Parse
  Check
  Compile
  Preprocess
  Lint
  General
}
```

## Working Rules
- Use `.moju/ai/ai-task.md` as the task source.
- Keep changes focused on relevant `.mju`, `layout.json`, or necessary documentation files.
- Do not introduce duplicate definitions.
- Run the relevant `moju verify .` / `moju readiness .`, or the project's existing validation command.
