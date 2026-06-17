# Ad Blocking

qbrsh blocks ads and trackers natively, in two layers, with no extension.

## How it works

**Navigation layer.** When the page navigates or loads a subframe, the engine's
navigation-policy handler checks the request host against the blocklist
synchronously and cancels it if blocked. This catches ad frames, popups, and
tracker redirects. It runs on the spot and never goes through the message loop,
so it adds no latency.

**Subresource layer.** At startup the blocklist is compiled into a WebKit
content filter (a Safari-style content blocker) and applied to every page. This
blocks matching subresource loads such as images, scripts, stylesheets, and
XHR before they reach the network. Compilation happens off the main thread, so
startup is not blocked.

## Customizing the list

qbrsh ships with a built-in set of common ad and tracker domains. Add your own
in `~/.local/share/qbrsh/adblock`, one domain per line:

```
ads.example.com
tracker.example.net
# lines starting with # are comments
```

A domain matches its exact host and any subdomain. Both layers use the same
list, so additions take effect for navigation immediately and for subresources
after the filter recompiles.

::: tip
The built-in list is intentionally small and curated. Pointing qbrsh at a large
community filter list is a planned enhancement.
:::
