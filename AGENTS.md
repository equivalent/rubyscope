# Rules for agents working on rubyscope

This repo is public. rubyscope is tested on private Rails apps. Nothing about
those apps may get into a committed file.

- Never name a real project, company, client or private repo in code, tests,
  docs, plans, commit messages or PR text.
- Never copy code, file paths, class names or data from a real project into
  tests or samples. Write made-up examples instead (for example `Order`,
  `Customer`, `app/models/order.rb`).
- Do not give facts that point to a real project, such as its file count,
  gem count, memory use or exact folder layout. Say "a large Rails app".
- Real click points and probe output stay in ignored files (see
  `.gitignore`), never in the repo.
- Never commit secrets: keys, tokens, passwords, `.env` files, or private
  local paths such as `/home/<user>/...`.
- Before each commit, check the staged diff for all of the above.

## Run bin/ci before every push

This repo has no CI server. `bin/ci` runs the same checks a CI server would:

1. formatting (`cargo fmt --check`)
2. lints, with warnings as errors (`cargo clippy -D warnings`)
3. all tests, with the window tests (`cargo test`)
4. Ruby syntax of `probe/rails_probe.rb`
5. no private local paths or `.env` files in the repo

Rules:

- Run `bin/ci` before each push to any branch. Push only when it ends with
  `bin/ci passed`.
- When a step fails, fix the cause and run `bin/ci` again. Do not skip a
  step, and do not remove or weaken a test to make it pass.
- If a failure is not caused by your change, stop and tell the user. Do not
  push.
- In the PR text or your report, say that `bin/ci` passed.
