---- MODULE MC ----
\* Model-checking configuration for DeploymentEngine.
EXTENDS DeploymentEngine, TLC

CONSTANTS r1, r2, r3

const_AllResources == {r1, r2, r3}

\* Dependency chain: r3 depends on r2, r2 depends on r1, r1 has no deps.
const_Deps == (r1 :> {} @@ r2 :> {r1} @@ r3 :> {r2})

====
