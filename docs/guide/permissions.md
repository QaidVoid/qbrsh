# Permissions

When a page asks for geolocation, notifications, or media access, qbrsh decides
based on a per-site policy rather than prompting or always denying.

## Policies

Each site resolves to one of three policies:

- `allow`: grant the request.
- `deny`: refuse the request.
- `ask`: currently falls back to deny, because there is no prompt UI yet.

The default policy is `deny`. Permission decisions are made synchronously in the
engine, keyed by the requesting page's host.

## Configuring

Set a default and per-site overrides in `config.toml`:

```toml
[permissions]
default = "deny"

[permissions.sites]
"maps.example.com" = "allow"
"ads.example.net" = "deny"
```

A site rule matches the exact host or any subdomain of it, so a rule for
`example.com` also covers `app.example.com`.

## At runtime

Grant or revoke without editing the file:

```
:set permissions.maps.example.com allow
:set permissions.default deny
```

Run `:config-source` to reload the file later.
