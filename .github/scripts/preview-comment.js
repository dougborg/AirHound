// Post or update a PR comment with download links for all bin-* artifacts.
// Called by actions/github-script in the preview-comment CI job.
// Automatically discovers artifacts — no changes needed when adding new boards.

const MARKER = "## Preview Binaries";

/**
 * Build the comment body from a list of artifacts and job results.
 * Pure function — no API calls, fully testable.
 */
function buildCommentBody({ artifacts, needs, runUrl }) {
  const artifactBaseUrl = `${runUrl}/artifacts`;

  // Filter for bin-* artifacts (flashable binaries) and sort by name
  const bins = artifacts
    .filter((a) => a.name.startsWith("bin-"))
    .sort((a, b) => a.name.localeCompare(b.name));

  // Auto-group: artifacts with -std suffix are std/ESP-IDF, rest are no_std/Embassy
  const noStd = bins.filter((a) => !a.name.endsWith("-std"));
  const stdBins = bins.filter((a) => a.name.endsWith("-std"));

  function buildTable(items) {
    const rows = [
      "| Artifact | Size | Download |",
      "|----------|------|----------|",
    ];
    for (const a of items) {
      const sizeMB = (a.size_in_bytes / (1024 * 1024)).toFixed(1);
      rows.push(
        `| \`${a.name}\` | ${sizeMB} MB | [Download](${artifactBaseUrl}/${a.id}) |`
      );
    }
    return rows.join("\n");
  }

  const lines = [MARKER, ""];

  if (bins.length > 0) {
    lines.push("Flashable firmware binaries for this PR:", "");

    if (noStd.length > 0) {
      lines.push("#### no_std (Embassy)", "", buildTable(noStd), "");
    }
    if (stdBins.length > 0) {
      lines.push("#### std (ESP-IDF)", "", buildTable(stdBins), "");
    }
  }

  // Report failed upstream jobs
  const failed = Object.entries(needs)
    .filter(([_, v]) => v.result !== "success" && v.result !== "skipped")
    .map(([name]) => name);

  if (failed.length > 0) {
    lines.push(
      `> **Build failures:** ${failed.join(", ")} — see [workflow run](${runUrl})`
    );
  } else if (bins.length === 0) {
    lines.push("No preview binaries available for this run.");
  }

  return lines.join("\n");
}

/**
 * Main entry point — called by actions/github-script.
 */
async function run({ github, context, needs }) {
  const { owner, repo } = context.repo;
  const runId = context.runId;
  const runUrl = `https://github.com/${owner}/${repo}/actions/runs/${runId}`;

  // Fetch all artifacts for this workflow run
  const {
    data: { artifacts },
  } = await github.rest.actions.listWorkflowRunArtifacts({
    owner,
    repo,
    run_id: runId,
    per_page: 100,
  });

  const body = buildCommentBody({ artifacts, needs, runUrl });

  // Upsert: find existing comment by marker, update or create
  const { data: comments } = await github.rest.issues.listComments({
    owner,
    repo,
    issue_number: context.issue.number,
  });
  const existing = comments.find((c) => c.body?.startsWith(MARKER));
  if (existing) {
    await github.rest.issues.updateComment({
      owner,
      repo,
      comment_id: existing.id,
      body,
    });
  } else {
    await github.rest.issues.createComment({
      owner,
      repo,
      issue_number: context.issue.number,
      body,
    });
  }
}

module.exports = run;
module.exports.buildCommentBody = buildCommentBody;
module.exports.MARKER = MARKER;
