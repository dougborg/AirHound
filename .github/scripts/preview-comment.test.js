const { describe, it } = require("node:test");
const assert = require("node:assert/strict");
const run = require("./preview-comment");
const { buildCommentBody, MARKER } = run;

const RUN_URL = "https://github.com/owner/repo/actions/runs/123";

function artifact(name, id, sizeBytes) {
  return { name, id, size_in_bytes: sizeBytes };
}

const MB = 1024 * 1024;

describe("buildCommentBody", () => {
  it("lists no_std and std artifacts in separate sections", () => {
    const body = buildCommentBody({
      artifacts: [
        artifact("bin-m5stickc", 10, 1.5 * MB),
        artifact("bin-xiao", 11, 2.0 * MB),
        artifact("bin-xiao-std", 20, 3.0 * MB),
        artifact("bin-m5stickc-std", 21, 2.5 * MB),
      ],
      needs: {
        "preview-binaries": { result: "success" },
        "preview-binaries-std": { result: "success" },
      },
      runUrl: RUN_URL,
    });

    assert.ok(body.startsWith(MARKER));
    assert.ok(body.includes("#### no_std (Embassy)"));
    assert.ok(body.includes("#### std (ESP-IDF)"));
    // Sorted by name within each group
    assert.ok(body.indexOf("bin-m5stickc") < body.indexOf("bin-xiao"));
    assert.ok(body.indexOf("bin-m5stickc-std") < body.indexOf("bin-xiao-std"));
  });

  it("builds download links with artifact IDs", () => {
    const body = buildCommentBody({
      artifacts: [artifact("bin-xiao", 42, 1 * MB)],
      needs: { "preview-binaries": { result: "success" } },
      runUrl: RUN_URL,
    });

    assert.ok(body.includes(`[Download](${RUN_URL}/artifacts/42)`));
  });

  it("formats artifact sizes in MB", () => {
    const body = buildCommentBody({
      artifacts: [artifact("bin-xiao", 1, 1.5 * MB)],
      needs: { "preview-binaries": { result: "success" } },
      runUrl: RUN_URL,
    });

    assert.ok(body.includes("1.5 MB"));
  });

  it("shows only no_std section when no std artifacts exist", () => {
    const body = buildCommentBody({
      artifacts: [artifact("bin-xiao", 1, 1 * MB)],
      needs: { "preview-binaries": { result: "success" } },
      runUrl: RUN_URL,
    });

    assert.ok(body.includes("#### no_std (Embassy)"));
    assert.ok(!body.includes("#### std (ESP-IDF)"));
  });

  it("shows only std section when no no_std artifacts exist", () => {
    const body = buildCommentBody({
      artifacts: [artifact("bin-xiao-std", 1, 1 * MB)],
      needs: { "preview-binaries-std": { result: "success" } },
      runUrl: RUN_URL,
    });

    assert.ok(!body.includes("#### no_std (Embassy)"));
    assert.ok(body.includes("#### std (ESP-IDF)"));
  });

  it("ignores non-bin artifacts (elf-*, etc.)", () => {
    const body = buildCommentBody({
      artifacts: [
        artifact("elf-xiao", 1, 10 * MB),
        artifact("elf-m5stickc", 2, 10 * MB),
        artifact("bin-xiao", 3, 1 * MB),
      ],
      needs: { "preview-binaries": { result: "success" } },
      runUrl: RUN_URL,
    });

    assert.ok(!body.includes("elf-xiao"));
    assert.ok(!body.includes("elf-m5stickc"));
    assert.ok(body.includes("bin-xiao"));
  });

  it("reports failed jobs", () => {
    const body = buildCommentBody({
      artifacts: [artifact("bin-xiao", 1, 1 * MB)],
      needs: {
        "preview-binaries": { result: "success" },
        "preview-binaries-std": { result: "failure" },
      },
      runUrl: RUN_URL,
    });

    assert.ok(body.includes("**Build failures:** preview-binaries-std"));
    assert.ok(body.includes(`[workflow run](${RUN_URL})`));
  });

  it("reports multiple failed jobs", () => {
    const body = buildCommentBody({
      artifacts: [],
      needs: {
        "preview-binaries": { result: "failure" },
        "preview-binaries-std": { result: "failure" },
      },
      runUrl: RUN_URL,
    });

    assert.ok(
      body.includes("**Build failures:** preview-binaries, preview-binaries-std")
    );
  });

  it("treats skipped jobs as non-failures", () => {
    const body = buildCommentBody({
      artifacts: [artifact("bin-xiao", 1, 1 * MB)],
      needs: {
        "preview-binaries": { result: "success" },
        "preview-binaries-std": { result: "skipped" },
      },
      runUrl: RUN_URL,
    });

    assert.ok(!body.includes("Build failures"));
  });

  it("shows empty message when no artifacts and no failures", () => {
    const body = buildCommentBody({
      artifacts: [],
      needs: {
        "preview-binaries": { result: "skipped" },
        "preview-binaries-std": { result: "skipped" },
      },
      runUrl: RUN_URL,
    });

    assert.ok(body.includes("No preview binaries available"));
  });

  it("picks up new board artifacts automatically", () => {
    const body = buildCommentBody({
      artifacts: [
        artifact("bin-xiao", 1, 1 * MB),
        artifact("bin-m5stickc", 2, 1 * MB),
        artifact("bin-new-board", 3, 1 * MB),
        artifact("bin-new-board-std", 4, 1 * MB),
        artifact("bin-xiao-std", 5, 1 * MB),
      ],
      needs: {
        "preview-binaries": { result: "success" },
        "preview-binaries-std": { result: "success" },
      },
      runUrl: RUN_URL,
    });

    // All 5 artifacts appear
    assert.ok(body.includes("bin-new-board"));
    assert.ok(body.includes("bin-new-board-std"));
    // new-board (no -std suffix) is in no_std section
    const noStdStart = body.indexOf("#### no_std");
    const stdStart = body.indexOf("#### std");
    const newBoardPos = body.indexOf("`bin-new-board`");
    const newBoardStdPos = body.indexOf("`bin-new-board-std`");
    assert.ok(newBoardPos > noStdStart && newBoardPos < stdStart);
    assert.ok(newBoardStdPos > stdStart);
  });
});

describe("run (integration)", () => {
  it("creates a new comment when none exists", async () => {
    const created = [];
    const github = {
      rest: {
        actions: {
          listWorkflowRunArtifacts: async () => ({
            data: { artifacts: [artifact("bin-xiao", 1, 1 * MB)] },
          }),
        },
        issues: {
          listComments: async () => ({ data: [] }),
          createComment: async (args) => created.push(args),
        },
      },
    };
    const context = {
      repo: { owner: "o", repo: "r" },
      runId: 100,
      issue: { number: 5 },
    };

    await run({
      github,
      context,
      needs: { "preview-binaries": { result: "success" } },
    });

    assert.equal(created.length, 1);
    assert.equal(created[0].issue_number, 5);
    assert.ok(created[0].body.startsWith(MARKER));
  });

  it("updates an existing comment", async () => {
    const updated = [];
    const github = {
      rest: {
        actions: {
          listWorkflowRunArtifacts: async () => ({
            data: { artifacts: [artifact("bin-xiao", 1, 1 * MB)] },
          }),
        },
        issues: {
          listComments: async () => ({
            data: [{ id: 99, body: `${MARKER}\nold content` }],
          }),
          updateComment: async (args) => updated.push(args),
        },
      },
    };
    const context = {
      repo: { owner: "o", repo: "r" },
      runId: 100,
      issue: { number: 5 },
    };

    await run({
      github,
      context,
      needs: { "preview-binaries": { result: "success" } },
    });

    assert.equal(updated.length, 1);
    assert.equal(updated[0].comment_id, 99);
    assert.ok(updated[0].body.includes("bin-xiao"));
  });
});
