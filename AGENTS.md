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
