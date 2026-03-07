# Skyr

## Introduction

Skyr is an infrastructure and container orchestrator that uses Git as its interface and utilizes a reconciliation protocol to self-heal.
Configuration is expressed in a rich DSL which allows modelling the entire infrastructure as a DAG expressed in code.
Pushing code to the Git repository hosted by Skyr (the Skyr Configuration Server, or SCS) causes a deployment of the corresponding infra to be created, and subsequently rolled out.

### High Level Example

Starting with an empty Git repository, we can create a file `Main.scl` with the following Skyr Configuration Language (SCL) contents:

```
import Std/Random
import Std/Artifact

a = Random.Int(name: "a", min: 0, max: 10)
b = Random.Int(name: "b", min: 0, max: 10)
c = Random.Int(name: "c", min: a.result, max: 20)

Artifact.File(name: "result.txt", contents: "The result is {a.result + b.result + c.result}")
```

This configuration derives a `Artifact.File` resource which is a downloadable file which will be exposed by Skyr.
This particular file is named `result.txt`.
The contents of the file is derived from the sum of three integers, derived by three individual `Random.Int` resources.
Looking closer at the integers, we can see that while `a` and `b` are configured to be between 0 and 10, `c` is configured to have the lower bound of whatever the result of `a` is.
This makes `c` dependent on `a` just like the file artifact is dependent on all three integers.

These resources form a directed acyclic graph (DAG) in which the artifact is a "root", meaning it doesn't have any dependents.
The integers `a` and `b` are "leaves", meaning they don't have any dependency on other resources – they are configured purely by static inputs.
The integer `c` is neither a root nor a leaf, since it is dependent on `a`, and is a dependency to the artifact. 

If we commit this `Main.skyr` file to the `main` branch, and push to Skyr, it will create a new "deployment" of the `main` branch for this commit.
If the ref (branch) is configured to "auto-rollout", the deployment is created with the `DESIRED` state, otherwise it's created as `UNDESIRED`.
This allows developers to manually trigger a deployment rollout by making the deployment `DESIRED`.
This will signal the Deployment Engine (DE) to start processing it.
The DE will start by executing the configuration, top-to-bottom.

- `import Std/Random` – this will import the `Random` module from the standard library.
- `import Std/Artifact` – similarly, this will import the `Artifact` module.
- `a = Random.Int(name: "a", min: 0, max: 10)` – while interpreting the call to `Random.Int`, the runtime will find that it has all the necessary inputs to derive the resource.
  Being a resource, `Random.Int` must have a way of deriving a globally unique identifier for itself based solely on its inputs.
  This is why a `name` parameter is necessary.
  It constructs the UID `Std/Random.Int:a` composed by the module name, resource name, and a resource ID.
  In this case, the resource used the provided `name` as a resource ID, but other resource types may choose any other strategy, as long as the ID is derived from the input arguments provided to the resource.
  Armed with the UID, the runtime looks up the resource in a repository-wide Resource Database (RD).
  Since this is the first run, the resource doesn't exist.
  The runtime proceeds by placing a `CREATE` message on the Resource Transition Queue (RTQ) containing the resource type, ID, and its inputs, as well as the ID of the deployment which will "own" the resource.
  Since the runtime itself is not responsible for actually performing the creation of the resource, it uses a special "pending" value as a placeholder for the execution to continue.
  Additionally, it attaches to the value a marker that indicates that it is dependent on the `Std/Random.Int:a` resource.
  Proceeding with the assignment statement of the config code, the pending value is assigned to the variable `a` in the runtime.
- `b = Random.Int(name: "b", min: 0, max: 10)` – being virtually the same as the previous line, the resource `Std/Random.Int:b` will be created in the same way and a pending value is assigned to `b`.
- `c = Random.Int(name: "c", min: a.result, max: 10)` – while interpreting the call to `Random.Int`, the runtime will encounter the expression `a.result`.
  Since the `a` variable holds a pending value, the `result` property cannot be accessed.
  Instead, the whole expression will be coalesced into the pending value.
  Now, the parameter `min` of the `Random.Int` resource is given a pending value dependent on the `a` resource.
  This means the `c` resource doesn't have enough information to be created.
  So, another pending value is used, and `c` is not scheduled for creation.
- `Artifact.File(name: "result.txt", contents: "The result is {a.result + b.result + c.result}")` – predictably, this entire expression will result in another pending value, since neither of the random integers have been created yet.

As we can see, the execution of the configuration by the DE resulted in two `CREATE` messages being sent to the RTQ.
These messages are consumed by the Resource Transition Engine (RTE) which is a distributed set of workers.
As the DE keeps executing the same configuration (which it does repeatedly), the same messages will be emitted.
Therefore, the messages on the RTQ are treated as idempotent.
That is, only the first of the `CREATE` messages will be processed, and after creation, the subsequent copies will be dropped.
To prevent that two different workers on the RTE get two copies of the same `CREATE` message, the routing on the queue uses a hash of the resource UID, so that all `CREATE` messages arrive at the same worker.

While the RTQ is processed by the RTE, the deployment will keep executing the configuration over and over again every couple of seconds.
Each time, the RD is consulted for any new resource state.
Eventually, one of the RTE workers will pick the first `CREATE` message from the RTQ, and perform the actual creation of the resource.
In this example, the `Std/Random.Int:a` and `Std/Random.Int:b` resources are queued up, and there is no guarantee of the order of the two, since that depends on the level of congestion on the particular workers targeted by their UID hashes on the RTE.

After the RTE worker has generated, say, the `Std/Random.Int:b` resource, the RDB will be updated to reflect that.
Let's say the actual integer chosen by the RNG ends up being 7.
The next time the deployment is executed, it will find the resource in the RDB.
Subsequently, the value of the `a` variable will be assigned an actual value of `Random.Int{name: "a", min: 0, max: 10, result: 7}`.
Still, however, this value is marked as being dependent on the `a` resource.
This is used for dependency tracking.
As execution continues, we now find that `c` has all of its input arguments known, since the `min` argument now gets the 7 from the `a.result`.
Thus, another `CREATE` message can be submitted for the `Std/Random.Int:c` resource.
The message will also contain the information that the resource is dependent on `Std/Random.Int:a`.
A pending value is created, and marked as dependent on both `a` and `c`.

This cycle contines, with the deployment continuously emitting `CREATE` messages on the RTQ, and the RTE processing them idempotently.
After all random integers are created, the `Std/Artifact.File:result.txt` resource is scheduled for creation.
The RTE worker will create the resource with the given `contents` and save in the RDB.

Finally, one execution of the deployment will go through the entire configuration without encountering any pending resource transitions.
At this point, the deployment will be marked as "up".

The deployment now transitions its execution to "health check mode", wherein executions of the configuration includes checking that resources hasn't strayed from its configuration.
If it has, execution will emit a `RESTORE` message on the RTQ.

### A superceding deployment

We now change the `Main.skyr` file to the following:

```
import Std/Random
import Std/Artifact

b = Random.Int(name: "b", min: 0, max: 10)

Artifact.File(name: "result.txt", contents: "The result is {b.result}")
```

When we commit and push, a new deployment will be created.
Placed in the `DESIRED` state, the deployment will supercede the existing deployment if it's pushed on the same branch as before.
When this happens, the previous deployment will be placed in the `LINGER` state.

When marked `LINGER`, the execution will work as in the health check mode, except it will pay attention to the owning deployment of each resource.
If the owner is updated – a process we call adoption – then the lingering deployment will not try to `RESTORE` the resource.

The `DESIRED` execution works as described in the original outline, except it will notice that the `b` integer resource already exists under another owning deployment.
This will emit an `ADOPT` message on the RTQ.
When executed by the RTE, the adoption causes an update to be made of the original resource, with potentially changed inputs, and it will be marked as owned by the new adopting deployment.

Eventually, the new deployment is "up", and the lingering deployment is instead marked as `UNDESIRED`.
This will change the execution to no longer go through the configuration files, but instead simply look at the DAG of resources in the RDB.
By finding all the resources owned by the `UNDESIRED` deployment, that have no living dependents, these are marked for destruction via `DESTROY` messages on the RTQ.
The `DESTROY` messages are just as idempotent as the other types of messages.

When an execution of an `UNDESIRED` deployment finds no resources at all owned by the deployment, the deployment is marked as "down" and execution stops.
