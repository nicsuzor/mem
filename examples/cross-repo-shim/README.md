# Reusable PR Pipeline (Shim Installation)

This repository uses the reusable PR pipeline from `academicOps`.

## Components

The pipeline consists of four shim workflows:

1. **`pr-pipeline.yml`**: The project-specific CI orchestrator. It initializes the `merge-prep-status` and runs the local test suite.
2. **`agent-merge-prep.yml`**: A shim that calls the reusable Merge Prep Agent from `academicOps`.
3. **`merge-prep-cron.yml`**: A shim that calls the reusable Dispatcher from `academicOps`. It runs on a cron and on completion of the CI pipeline.
4. **`agent-enforcer.yml`**: A shim that calls the reusable Axiom Compliance Reviewer (Enforcer) from `academicOps`.

## Installation

To install this pipeline on a new repository:

1. Copy the shim files from `.github/workflows/` to your repository.
2. Ensure you have the following secrets configured (Note: `GITHUB_TOKEN` is provided by default):
   - `AOPS_BOT_GH_TOKEN`: A PAT with `contents: write`, `pull-requests: write`, `statuses: write`, and `actions: read/write`.
   - `CLAUDE_CODE_OAUTH_TOKEN`: OAuth token for the Claude agent.
3. Pin the workflows to a stable tag (e.g., `@pipeline-v1`) in the `uses:` declarations.

## Rollback Procedure

If a regression is identified in the imported `@pipeline-v1` workflows:

1. Identify the previous stable tag or SHA in the `academicOps` repository.
2. Update the `@pipeline-v1` reference in all shim files to the specific tag or SHA.
3. Commit and push the changes.

## Secret Rotation

To rotate the `AOPS_BOT_GH_TOKEN`:

1. Generate a new PAT with the required permissions.
2. Update the secret in the repository settings.
3. The pipeline will automatically use the new token on the next run.
