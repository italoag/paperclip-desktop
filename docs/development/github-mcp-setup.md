# Re-enabling the GitHub MCP server

The entry that shipped in `~/.config/devin/mcp_config.json` pointed at
`https://api.githubcopilot.com/mcp` with no credentials. That endpoint wants a
device-login flow it never surfaces a code for, so every session got 401/429 —
63 failed connections and 129 dead transport workers in one day of logs, on top
of the latency each attempt adds. It was removed; the backup sits next to the
file as `mcp_config.json.bak-<timestamp>`.

## The working alternative

GitHub's own server, run over stdio with a token. Verified against this machine:
`initialize` returns `github-mcp-server v1.10.1` and `tools/list` returns 44
tools.

```json
{
  "mcpServers": {
    "github": {
      "command": "docker",
      "args": [
        "run", "-i", "--rm",
        "-e", "GITHUB_PERSONAL_ACCESS_TOKEN",
        "ghcr.io/github/github-mcp-server", "stdio"
      ],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "<token>" }
    }
  }
}
```

Apply it with the token from the `gh` CLI, without the value ever reaching a
shell history or a log:

```sh
python3 - <<'PY'
import json, os, subprocess
path = os.path.expanduser("~/.config/devin/mcp_config.json")
config = json.load(open(path))
config.setdefault("mcpServers", {})["github"] = {
    "command": "docker",
    "args": ["run", "-i", "--rm", "-e", "GITHUB_PERSONAL_ACCESS_TOKEN",
             "ghcr.io/github/github-mcp-server", "stdio"],
    "env": {"GITHUB_PERSONAL_ACCESS_TOKEN":
            subprocess.check_output(["gh", "auth", "token"], text=True).strip()},
}
json.dump(config, open(path, "w"), indent=2)
os.chmod(path, 0o600)
PY
```

## What you are accepting by doing this

**A token in a file.** `mcp_config.json` is now `0600`, but it was
world-readable until this cleanup, so check nothing else copied it. The adapter
symlinks this file into every agent's Devin config home, which is the point —
the agents need GitHub access — but it does mean every agent can use that token
with its full scope. The `gh` token here carries `gist`, `read:org`, `repo` and
`workflow`. A fine-grained PAT limited to the repositories the agents actually
touch is the safer choice.

**It expires.** `gh` issues OAuth tokens that rotate. When it does, the MCP
starts failing the same way the old entry did. A long-lived fine-grained PAT
avoids that.

## Why not the npm package

`@modelcontextprotocol/server-github` is the obvious search result and it is
deprecated: "Package no longer supported." It still installs, which is what
makes it a trap.
