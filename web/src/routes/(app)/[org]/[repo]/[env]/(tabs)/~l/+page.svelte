<script lang="ts">
import { page } from "$app/stores";
import LogStream from "$lib/components/LogStream.svelte";
import { EnvironmentDetailDocument, EnvironmentLogsDocument } from "$lib/graphql/generated";
import { graphqlQuery } from "$lib/graphql/query";
import { decodeSegment } from "$lib/paths";

let orgName = $derived($page.params.org ?? "");
let repoName = $derived($page.params.repo ?? "");
let envName = $derived(decodeSegment($page.params.env ?? ""));

const envDetail = graphqlQuery(() => ({
    document: EnvironmentDetailDocument,
    variables: { org: orgName, repo: repoName, env: envName },
    refetchInterval: 10_000,
}));

let env = $derived(envDetail.data?.organization.repository.environment ?? null);
</script>

{#if env}
  <div
    class="h-[calc(100vh-16rem)] bg-gray-900 border border-gray-800 rounded-lg overflow-hidden"
  >
    <LogStream
      document={EnvironmentLogsDocument}
      variables={{ environmentQid: env.qid, initialAmount: 50 }}
      logField="environmentLogs"
    />
  </div>
{/if}
