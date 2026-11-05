// Cloudflare Worker in front of the GitHub Pages site (battery.faulcloud.work).
// GitHub Pages can't set headers or negotiate content, so this adds:
//  - Accept: text/markdown negotiation on pages: "/" returns /index.md to
//    clients that ask for Markdown, HTML to everyone else, 406 if neither fits.
//  - Link: rel="alternate" headers between each page and its Markdown twin.
//  - Vary: Accept, so caches keep the two representations apart.
// Assets (images, robots.txt, sitemap.xml, llms.txt) pass through untouched.

const PAGES = { "/": "/index.md", "/index.html": "/index.md" };

export default {
  async fetch(request) {
    const url = new URL(request.url);
    const path = url.pathname;

    if (path.endsWith(".md")) {
      const html = Object.keys(PAGES).find((p) => PAGES[p] === path) ?? "/";
      return markdown(await fetch(request), html);
    }

    const md = PAGES[path];
    if (!md) return fetch(request);

    const choice = negotiate(request.headers.get("Accept"));
    if (choice === null) {
      return new Response("Not Acceptable: this page is available as text/html or text/markdown.\n", {
        status: 406,
        headers: { "Content-Type": "text/plain; charset=utf-8", Vary: "Accept" },
      });
    }
    if (choice === "markdown") {
      const mdUrl = new URL(md, url);
      return markdown(await fetch(mdUrl, { headers: request.headers }), path);
    }

    const origin = await fetch(request);
    const res = new Response(origin.body, origin);
    res.headers.set("Link", `<${md}>; rel="alternate"; type="text/markdown"`);
    res.headers.append("Vary", "Accept");
    return res;
  },
};

function markdown(origin, htmlPath) {
  const res = new Response(origin.body, origin);
  if (origin.ok) res.headers.set("Content-Type", "text/markdown; charset=utf-8");
  res.headers.set("Link", `<${htmlPath}>; rel="alternate"; type="text/html"`);
  res.headers.append("Vary", "Accept");
  return res;
}

// Returns "markdown", "html", or null (406). Compares q-values; a tie goes to
// Markdown only when the client named text/markdown explicitly, so a browser's
// */* never flips to Markdown.
export function negotiate(accept) {
  if (!accept || !accept.trim()) return "html";
  const ranges = accept.split(",").map((part) => {
    const [type, ...params] = part.trim().toLowerCase().split(";");
    let q = 1;
    for (const p of params) {
      const [k, v] = p.trim().split("=");
      if (k === "q") q = Number.parseFloat(v);
    }
    return { type: type.trim(), q: Number.isFinite(q) ? q : 0 };
  });
  const qFor = (mime) => {
    const [major] = mime.split("/");
    let best = null; // most specific match wins, per RFC 9110
    for (const r of ranges) {
      const rank = r.type === mime ? 3 : r.type === `${major}/*` ? 2 : r.type === "*/*" ? 1 : 0;
      if (rank && (!best || rank > best.rank)) best = { rank, q: r.q };
    }
    return best ? best.q : 0;
  };
  const md = qFor("text/markdown");
  const html = qFor("text/html");
  const explicitMd = ranges.some((r) => r.type === "text/markdown" && r.q > 0);
  if (explicitMd && md >= html) return "markdown";
  if (html > 0) return "html";
  if (md > 0) return "markdown";
  return null;
}
