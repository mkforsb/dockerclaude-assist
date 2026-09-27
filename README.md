# dockerclaude

## How to copy auth from host machine

1. Copy `~/.claude/.credentials.json` to the dockerclaude `.claude` folder.
2. Copy keys from `~/.claude.json` to dockerclaude `.claude/.claude.json`:
  - `"hasCompletedOnboarding": true`
  - `"oauthAccount": { ... }`
