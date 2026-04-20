---- MODULE DeploymentEngine ----
\* TLA+ specification of the Deployment Engine v2 lifecycle model.
\* See DE_V2.md for the design document.

EXTENDS Naturals, FiniteSets

CONSTANTS
    MaxDeployments,  \* Maximum number of deployments (finite for model checking)
    AllResources     \* Set of all possible resource identifiers

VARIABLES
    state,          \* Lifecycle label per deployment: "none" | "DESIRED" | "LINGERING" | "UNDESIRED" | "DOWN"
    boot,           \* Bootstrapped flag per deployment
    res,            \* Resource set per deployment (R(µ) in the design doc)
    sup,            \* Supersession: maps deployment to its successor (0 = none)
    num,            \* Number of deployments created so far
    alive           \* Set of resources that currently exist in reality

vars == <<state, boot, res, sup, num, alive>>

Ids == 1..MaxDeployments
Labels == {"none", "DESIRED", "LINGERING", "UNDESIRED", "DOWN"}

\* ---------------------------------------------------------------------------
\* Derived state
\* ---------------------------------------------------------------------------

Superseded(d) == sup[d] # 0

\* Current deployment (the unsuperseded one), or 0 if none exist.
Current ==
    IF num = 0 THEN 0
    ELSE CHOOSE d \in 1..num : ~Superseded(d)

CurrentBootstrapped ==
    Current # 0 /\ boot[Current]

CurrentResources ==
    IF Current = 0 THEN {} ELSE res[Current]

\* Resources a deployment must destroy during teardown.
Teardown(d) == res[d] \ CurrentResources

\* ---------------------------------------------------------------------------
\* Initial state
\* ---------------------------------------------------------------------------

Init ==
    /\ state = [d \in Ids |-> "none"]
    /\ boot  = [d \in Ids |-> FALSE]
    /\ res   = [d \in Ids |-> {}]
    /\ sup   = [d \in Ids |-> 0]
    /\ num   = 0
    /\ alive = {}

\* ---------------------------------------------------------------------------
\* Actions
\* ---------------------------------------------------------------------------

\* A new deployment arrives with an arbitrary set of resources.
CreateDeployment ==
    /\ num < MaxDeployments
    /\ \E resources \in SUBSET AllResources :
        LET new == num + 1 IN
        /\ state' = [state EXCEPT ![new] = "DESIRED"]
        /\ boot'  = boot
        /\ res'   = [res EXCEPT ![new] = resources]
        /\ sup'   = IF num > 0
                     THEN [sup EXCEPT ![Current] = new]
                     ELSE sup
        /\ num'   = new
        /\ UNCHANGED alive

\* DESIRED: create a resource that should exist but doesn't yet.
DesiredCreate(d) ==
    /\ state[d] = "DESIRED"
    /\ ~Superseded(d)
    /\ \E r \in res[d] \ alive :
        /\ alive' = alive \cup {r}
        /\ UNCHANGED <<state, boot, res, sup, num>>

\* DESIRED: all resources converged — become bootstrapped.
DesiredBootstrap(d) ==
    /\ state[d] = "DESIRED"
    /\ ~Superseded(d)
    /\ res[d] \subseteq alive
    /\ ~boot[d]
    /\ boot' = [boot EXCEPT ![d] = TRUE]
    /\ UNCHANGED <<state, res, sup, num, alive>>

\* DESIRED -> LINGERING on supersession.
DesiredToLingering(d) ==
    /\ state[d] = "DESIRED"
    /\ Superseded(d)
    /\ state' = [state EXCEPT ![d] = "LINGERING"]
    /\ UNCHANGED <<boot, res, sup, num, alive>>

\* LINGERING -> UNDESIRED when Current is bootstrapped.
LingeringToUndesired(d) ==
    /\ state[d] = "LINGERING"
    /\ CurrentBootstrapped
    /\ state' = [state EXCEPT ![d] = "UNDESIRED"]
    /\ UNCHANGED <<boot, res, sup, num, alive>>

\* UNDESIRED: destroy a resource no longer desired by Current.
UndesiredDestroy(d) ==
    /\ state[d] = "UNDESIRED"
    /\ \E r \in Teardown(d) \cap alive :
        /\ alive' = alive \ {r}
        /\ UNCHANGED <<state, boot, res, sup, num>>

\* UNDESIRED -> DOWN when teardown is complete.
UndesiredToDown(d) ==
    /\ state[d] = "UNDESIRED"
    /\ Teardown(d) \cap alive = {}
    /\ state' = [state EXCEPT ![d] = "DOWN"]
    /\ UNCHANGED <<boot, res, sup, num, alive>>

\* ---------------------------------------------------------------------------
\* Specification
\* ---------------------------------------------------------------------------

Next ==
    \/ CreateDeployment
    \/ \E d \in 1..num :
        \/ DesiredCreate(d)
        \/ DesiredBootstrap(d)
        \/ DesiredToLingering(d)
        \/ LingeringToUndesired(d)
        \/ UndesiredDestroy(d)
        \/ UndesiredToDown(d)

Spec == Init /\ [][Next]_vars

\* Fair specification for liveness checking.
\* No fairness on CreateDeployment — it models an external event.
FairSpec == Spec /\ \A d \in Ids :
    /\ WF_vars(DesiredCreate(d))
    /\ WF_vars(DesiredBootstrap(d))
    /\ WF_vars(DesiredToLingering(d))
    /\ WF_vars(LingeringToUndesired(d))
    /\ WF_vars(UndesiredDestroy(d))
    /\ WF_vars(UndesiredToDown(d))

\* ---------------------------------------------------------------------------
\* Invariants (safety)
\* ---------------------------------------------------------------------------

TypeOK ==
    /\ state \in [Ids -> Labels]
    /\ boot  \in [Ids -> BOOLEAN]
    /\ res   \in [Ids -> SUBSET AllResources]
    /\ sup   \in [Ids -> 0..MaxDeployments]
    /\ num   \in 0..MaxDeployments
    /\ alive \subseteq AllResources

\* A bootstrapped Current deployment's resources are always alive.
\* UNDESIRED deployments only destroy Teardown(d) = R(d) \ R(Current),
\* so Current's resources are never touched.
CurrentResourcesSafe ==
    CurrentBootstrapped => CurrentResources \subseteq alive

\* No two deployments can simultaneously create/adopt resources.
\* Only a DESIRED, un-superseded (Current) deployment operates on resources,
\* and there can be at most one such deployment at any time.
NoResourceContention ==
    \A d1, d2 \in 1..num :
        (state[d1] = "DESIRED" /\ ~Superseded(d1) /\
         state[d2] = "DESIRED" /\ ~Superseded(d2))
        => d1 = d2

\* ---------------------------------------------------------------------------
\* Temporal properties (require FairSpec)
\* ---------------------------------------------------------------------------

\* Every superseded deployment eventually reaches DOWN.
AllSupersededReachDown ==
    \A d \in Ids :
        (state[d] # "none" /\ Superseded(d)) ~> (state[d] = "DOWN")

====
