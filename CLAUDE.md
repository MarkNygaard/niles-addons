@AGENTS.md

# Claude Code notes

Everything above comes from `AGENTS.md`, which every agent reads. What follows is only for Claude Code.

Commits end with:

```
Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

PR bodies end with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.

Write commit messages and PR bodies to a file with the Write tool and pass the path (`git commit -F <file>`, `gh pr create --body-file <file>`). In Bash, no heredocs, shell loops or `$(...)`.
