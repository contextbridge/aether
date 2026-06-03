Allow only a few read-only tools:

```json
{ "allow": ["read_file", "grep", "find"] }
```

Allow everything except the shell, using a trailing `*` wildcard:

```json
{ "deny": ["bash*"] }
```
