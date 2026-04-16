# Propositional Type Refinement

This design document outlines a powerful strategy to get intuitive type refinement from simple propositional logic carried through the existing type infrastructure.

Currently, the Type struct carries a kind, as well as an optional name, which is not taken into account for type equality.

Following that pattern, we're going to add some more metadata to the Type struct, namely "id". All types that are constructed get unique TypeIds that are generated on the fly, just like they are for type variables.

We can short-circuit type equality checks if the IDs are the same, and fall back to checking equality of kinds.

Then, we define a super simple propositional grammar:

enum Prop {
  IsTrue(TypeId),
  RefinesTo(TypeId, Type),
  Not(Box<Prop>),
  Implies(Box<Prop>, Box<Prop>),
}

`RefinesTo(id, ty)` means "the type with the given ID can be replaced with `ty`". This decouples the proposition system from any specific type pattern (like optionals) — the operator that emits the proposition is responsible for computing the refined type, and the derivation engine simply performs the substitution.

The intuition for this feature is:

Checking expressions results in propositions representing the relationships between underlying values and types. When entering contexts in which a new proposition proves the possible refinement of a type, that refinement is added to that context.

Here's an example:

```scl
// The binding `x` is annotated, so the `Int?` resolves to Optional(Int).
// The inner Int gets TypeId 1, the Optional(Int) gets TypeId 2.
let x: Int? = ...

// The `!=` operator sees that the comparison is with `nil`, so the
// Bool result gets TypeId 3, and the operator emits the proposition:
//   Implies(IsTrue(3), RefinesTo(2, Int(1)))
// meaning "if y is true, then x's optional type can be replaced with Int".
let y = x != nil

// In an if expression, we take the type of the condition (TypeId 3)
// and assume IsTrue(3) in the consequent, Not(IsTrue(3)) in the else.
if (y)
  // Propositions in scope:
  //   1) Implies(IsTrue(3), RefinesTo(2, Int(1)))  — from the != nil
  //   2) IsTrue(3)                                  — from the consequent assumption
  //   3) RefinesTo(2, Int(1))                       — derived via modus ponens (1 + 2)
  // Variable resolution: x's type (TypeId 2) is refined to Int(1).
  // x : Int
  x
else
  // Only Not(IsTrue(3)) is assumed here, which does not match the
  // antecedent of the implication, so no refinement is derived.
  // x : Int?
  x
```
  
## Proposition Storage and Propagation

Propositions are stored as a field on `TypeEnv` (or `TypeEnvMaps`), representing facts that are true within a given scope. Expression synthesis returns new propositions alongside the type, and the caller decides whether and how to add them to a child environment.

Propositions are stack-scoped to avoid excessive copying — each `TypeEnv` is bound to the call stack, and child environments are created on the stack with additional propositions applied.

`Prop` owns its `Type` values (inside `RefinesTo`), but `TypeEnv` borrows propositions rather than owning them — fitting the existing `'a` lifetime pattern. Expression synthesis returns owned propositions that live on the caller's stack; child `TypeEnv`s borrow references to them. The derivation engine produces new (derived) propositions that also need to be owned somewhere — a derivation result struct on the stack, with `TypeEnv` borrowing it.

### If-Expression Propagation

The if-expression is the primary site where propositions are introduced and turned into implications:

1. Check the condition expression, which returns propositions.
2. Create an inner env on the stack with the returned propositions applied, plus the assumption `IsTrue(condition_type_id)`. Use this env when synthesizing/checking the consequent branch.
3. Create another inner env on the stack with the returned propositions applied, plus the assumption `Not(IsTrue(condition_type_id))`. Use this env when checking the else branch.
4. Take propositions returned from both branches and wrap all of them in implications: the assumptions made in each branch become the antecedent, so what was assumed implies the propositions derived from that branch.

This means the if-expression itself returns implications as its output propositions, preserving the logical relationship without asserting either branch's assumptions unconditionally.

### Proposition Derivation

When propositions are applied to a child `TypeEnv`, all consequences are derived eagerly via forward-chaining (modus ponens over implications). The fully derived set of proven propositions — including the `RefinesTo` map — is stored in the `TypeEnv`.

### Refinement at Variable Resolution

Type refinement is applied **at variable resolution time**, not when entering a scope. When a variable is looked up, the proven `RefinesTo` map is consulted and applied **recursively** to the returned type: the substitution walks the type tree, replacing any type whose TypeId matches a proven `RefinesTo(id, ty)` with `ty`, then continues walking into the replacement (since the replacement may itself contain TypeIds that are also refined). This fixed-point application ensures that nested refinements compose correctly — e.g., a variable bound to a record with a refined optional field gets the field unwrapped when the variable's type is resolved.

This approach avoids the upfront cost of walking all locals when entering a scope, and naturally handles intermediate TypeIds (like `?.` result wrappers) that exist only in the proposition chain but never appear in a local's type. It also ensures the LSP cursor tracking sees fully refined types, since variable resolution is what feeds types into the cursor.

### TypeId Semantics

TypeIds track the *origin of a value*, not the binding name. They are only freshly minted at construction sites: literals, operator results, type annotations, and other expression forms that produce new values. Variable references and assignments propagate the TypeId from their source — they do not mint new IDs.

This means that if two bindings share the same TypeId (e.g., `let y = x`), refining one by ID refines both, which is correct: they alias the same underlying value.

### TypeId on the Type Struct

Every `Type` carries a required `id: TypeId` field (a `usize`). IDs are always freshly minted by default — reusing an existing ID is the explicit, deliberate choice.

Note: the `TypeId` is distinct from the ID in `TypeKind::Var(usize)`. A type variable's ID determines proper type identity (it participates in unification and assignability). A `TypeId`, by contrast, tracks the type's *origin* for propositional reasoning — it has no effect on assignability or equality. They may share the same counter for convenience, but they serve fundamentally different purposes. This biases toward correctness: it is better to incorrectly create a fresh ID (missing a refinement opportunity) than to incorrectly reuse one (applying a refinement where it shouldn't hold). Making ID reuse explicit also keeps the flow of value identity visible in the code.

### Derivation Engine

When entering a scope with new propositions, all consequences are derived eagerly via forward-chaining. Implications are indexed by their antecedent in a `HashMap<Prop, Vec<Prop>>`. When a new atomic proposition is proven:

1. Look up its consequents in the index.
2. For each consequent, add it to the proven set.
3. Recursively prove any further consequences triggered by the newly proven proposition.

This avoids rescanning all implications on each iteration and makes transitive chains (like the `z?.x` example, which requires chaining two implications) a simple recursive walk.

Once the proven set is fully derived, it is stored in the `TypeEnv` for use during variable resolution (see "Refinement at Variable Resolution" above). `Not(RefinesTo(...))` may appear in implication chains but is not actionable — it simply doesn't trigger any type substitution.

### Logical Operators

**`!` (NOT):**

The `!` operator emits a biconditional on `IsTrue`, since boolean inversion is symmetric:

- `!x` (result TypeId `r`, operand TypeId `x`) emits:
  - `Implies(IsTrue(r), Not(IsTrue(x)))`
  - `Implies(Not(IsTrue(r)), IsTrue(x))`

This ensures refinement works through negation in both branches of an if-expression (e.g., `if (!b)` refines in the else branch via the second implication).

**`&&` (AND):**

`a && b` (result TypeId `r`, operand TypeIds `a`, `b`) emits:
- `Implies(IsTrue(r), IsTrue(a))`
- `Implies(IsTrue(r), IsTrue(b))`

This gives conjunction refinement in if-consequents: `if (x != nil && y != nil)` refines both `x` and `y` in the then-branch.

`&&` checks its RHS in a child environment where `IsTrue(lhs_id)` is assumed (with propositions from the LHS applied). This matches short-circuit evaluation semantics and allows patterns like `z != nil && z.x > 0` where the RHS relies on the LHS having refined `z`. Propositions returned from the RHS are wrapped in `Implies(IsTrue(lhs_id), ...)` since they were derived under that assumption.

**`||` (OR):**

`a || b` (result TypeId `r`, operand TypeIds `a`, `b`) emits:
- `Implies(Not(IsTrue(r)), Not(IsTrue(a)))`
- `Implies(Not(IsTrue(r)), Not(IsTrue(b)))`

This gives disjunction refinement in else-branches: `if (x == nil || y == nil)` refines both `x` and `y` in the else-branch (where neither is nil).

Similarly, `||` checks its RHS in a child environment where `Not(IsTrue(lhs_id))` is assumed. Propositions from the RHS are wrapped in `Implies(Not(IsTrue(lhs_id)), ...)`.

### Nil Comparison Propositions

Nil comparisons emit `RefinesTo` propositions that carry the unwrapped type directly:

- `x != nil` where `x : Optional(inner)` (inner has TypeId `v`, optional has TypeId `o`, result has TypeId `r`) emits:
  - `Implies(IsTrue(r), RefinesTo(o, inner))`

- `x == nil` where `x : Optional(inner)` (same IDs) emits:
  - `Implies(Not(IsTrue(r)), RefinesTo(o, inner))`

Both forms reach `RefinesTo(o, inner)` through the if-expression's branch assumptions — `!= nil` via the consequent (`IsTrue`), `== nil` via the else branch (`Not(IsTrue)`).

### Optional Chaining Propositions

The `?.` operator creates a fresh `Optional` wrapper for the result, reusing the inner type's TypeId from the accessed field. It emits one or two implications depending on whether the field is itself optional:

**Always (source unwrap):** `Implies(RefinesTo(result, inner), RefinesTo(source, unwrapped_source))` — "if the result is refined, the source is non-nil."

**Only when the field is optional (field unwrap):** `Implies(RefinesTo(result, inner), RefinesTo(field_type, field_inner))` — "if the result is refined, the optional field is also non-nil."

When the field IS optional, `?.` wrapping and flattening produce a result whose inner type shares the TypeId with the field's inner type. When the field is NOT optional, the result's inner type shares the TypeId with the field type directly.

Example with non-optional field:

```
r : { f: Int }?  (1)
     { f: Int }  (2)
          Int    (3)

r?.f : Int?      (4)   — fresh wrapper
       Int       (3)   — reuses field's TypeId

Implies(RefinesTo(4, Int(3)), RefinesTo(1, { f: Int }(2)))   // source unwrap
```

Example with optional field:

```
r : { f: Int? }?  (1)
     { f: Int? }  (2)
          Int?     (3)
          Int      (4)

r?.f : Int?        (5)   — fresh wrapper
       Int         (4)   — reuses field's inner TypeId

Implies(RefinesTo(5, Int(4)), RefinesTo(1, { f: Int? }(2)))   // source unwrap
Implies(RefinesTo(5, Int(4)), RefinesTo(3, Int(4)))            // field unwrap
```

The derivation engine's antecedent index must support any `Prop` as a key, not just `IsTrue`-shaped propositions. When a `RefinesTo(r, ...)` is proven, it triggers implications keyed on that proposition, enabling transitive chains through nested optional chaining.

### Nil Coalesce (`??`)

The `??` operator unwraps an `Optional(inner)` at the type level — the result type propagates the inner TypeId from the optional. It does **not** emit any propositions: `x ?? y` does not prove that `x` is non-nil (the default may have been used). It is purely a value-level operation with no propositional impact.

### Prop Equality

`Prop` must implement `Eq + Hash` for use as a `HashMap` key in the antecedent index. Equality is defined structurally on the `Prop` enum, but does *not* delegate to type equality for the `Type` inside `RefinesTo`. Instead, `RefinesTo(a, ty1) == RefinesTo(b, ty2)` iff `a == b` and `ty1.id == ty2.id`. This keeps equality a simple comparison of `usize` pairs and avoids needing structural `Hash`/`Eq` on `TypeKind`.

### Proposition Forwarding

The default behavior for any expression is to forward all propositions from its sub-expressions, in addition to any propositions it emits itself. This ensures propositions from deeply nested sub-expressions bubble up to where they can be consumed.

The notable exception is `if`, which wraps sub-expression propositions from its branches in implications (as described above) rather than forwarding them directly.

### Proposition Accumulation Through Let Bindings

When checking `let x = a; b`, the propositions returned by `a` are added to the child environment used to check `b`. This means propositions accumulate naturally as let-bindings are sequenced — by the time a downstream expression (like an `if`) is reached, all propositions from prior bindings are in scope.

### Type Annotations and TypeId Boundaries

When a `let` binding has an explicit type annotation, the annotation creates a fresh TypeId. This makes annotations an explicit type boundary — propositions from the initializer expression still flow to the enclosing scope, but they refer to the expression's own TypeIds, not the annotated binding's. This is consistent with the "fresh by default, reuse is explicit" principle.

### Scope of Initial Implementation

The following constructs emit and consume propositions in the initial implementation:

- **Emitters:** `== nil`, `!= nil`, `!`, `&&`, `||`, `?.`
- **Consumers:** `if` (creates child environments with assumptions for each branch)

Other constructs (match expressions, function calls, early returns, etc.) do not participate in propositional refinement. Propositions do not cross function boundaries — a function cannot emit propositions about its arguments or return value to the caller.

## Examples

```scl
// Types: { x: Int }(1), Int(2), { x: Int }?(3)
let z: { x: Int }? = ...

// `?.` unwraps 3→1, accesses field `x` → Int(2) (not optional, so no field unwrap).
// Fresh result: Int?(4) wrapping Int(2). Emits:
//   Implies(RefinesTo(4, Int(2)), RefinesTo(3, { x: Int }(1)))   — source unwrap
let q = z?.x

// `!=` produces Bool(5). Emits:
//   Implies(IsTrue(5), RefinesTo(4, Int(2)))
let a = q != nil

// a : Bool (5)
if (a)
  // Derivation chain:
  //   IsTrue(5)                       — consequent assumption
  //   → RefinesTo(4, Int(2))          — from != nil
  //   → RefinesTo(3, { x: Int }(1))  — from ?. source unwrap
  // Variable resolution: z's type (3) is refined to { x: Int }(1).
  // z : { x: Int }, so z.x : Int
  z.x
```
