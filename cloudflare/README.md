# Cloudflare Worker for the website

GitHub Pages serves `docs/` as static files and can't set response headers, so
this optional Worker sits in front of battery.faulcloud.work and adds what AI
coding agents (Claude Code, Cursor, Copilot and others) look for:

- `Accept: text/markdown` on `/` returns the Markdown twin (`/index.md`),
  browsers still get HTML, and anything else gets `406 Not Acceptable`.
- `Link: </index.md>; rel="alternate"; type="text/markdown"` on the HTML, and
  a `Link` back to the HTML on the Markdown.
- `Vary: Accept` on both, so caches keep them apart.

Everything else is passed through to GitHub Pages unchanged.

## Turning it on

1. In Cloudflare DNS, switch the `battery` CNAME (to `faulik.github.io`) from
   DNS only (grey cloud) to Proxied (orange cloud), and set SSL/TLS to Full.
2. Under Security → Bots, make sure AI crawlers aren't blocked and the managed
   robots.txt is off, so `docs/robots.txt` is the one served.
3. Deploy: `cd cloudflare && npx wrangler deploy`.

Check it with:

```
curl -sI https://battery.faulcloud.work/ | grep -i -E '^(link|vary)'
curl -s -H 'Accept: text/markdown' https://battery.faulcloud.work/ | head -3
curl -s -o /dev/null -w '%{http_code}\n' -H 'Accept: image/png' https://battery.faulcloud.work/
```

or with <https://acceptmarkdown.com> and <https://isitagentready.com>.

`node cloudflare/negotiate.test.mjs` tests the `Accept` parsing.
