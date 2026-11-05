"""Generate the Markdown twin of docs/index.html.

The HTML page is the single source of truth. This writes docs/index.md and
docs/llms-full.txt from it, so agents that ask for Markdown get the same
content as browsers. Only the elements the page uses are handled.

Usage: python scripts/site_markdown.py [docs_dir]
"""

import re
import sys
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import urljoin

BASE = "https://battery.faulcloud.work/"
BLOCKS = {"p", "div", "footer", "h1", "h2", "h3", "dd", "dt", "table", "ul", "ol", "img"}


class ToMarkdown(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.out = []  # finished blocks
        self.buf = ""  # current inline text
        self.in_body = False
        self.skip = 0  # inside <style>/<script>
        self.href = None
        self.link_text = ""
        self.row = None
        self.rows = []
        self.heading = ""
        self.lists = []  # stack of item counters, None for <ul>
        self.label = False  # <b> opens a list item, as in the feature list

    # helpers
    def text(self, s):
        if self.href is not None:
            self.link_text += s
        elif self.row is not None:
            self.row[-1] += s
        else:
            self.buf += s

    def flush(self):
        line = " ".join(self.buf.split())
        if line:
            self.out.append(self.heading + line)
        self.buf = ""
        self.heading = ""

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "body":
            self.in_body = True
        if tag in ("style", "script"):
            self.skip += 1
        if not self.in_body or self.skip:
            return
        if tag in BLOCKS or tag == "li":
            self.flush()
        if tag in ("h1", "h2", "h3"):
            self.heading = "#" * int(tag[1]) + " "
        elif tag == "dt":
            self.heading = "### "
        elif tag in ("ul", "ol"):
            self.lists.append(0 if tag == "ol" else None)
        elif tag == "li":
            if self.lists and self.lists[-1] is not None:
                self.lists[-1] += 1
                self.buf = f"{self.lists[-1]}. "
            else:
                self.buf = "- "
        elif tag == "b":
            self.label = self.buf.strip() == "-"
            self.text("**")
        elif tag == "code":
            self.text("`")
        elif tag == "a":
            self.href = urljoin(BASE, a.get("href", ""))
            self.link_text = ""
        elif tag == "img":
            self.out.append(f"![{a.get('alt', '')}]({urljoin(BASE, a.get('src', ''))})")
        elif tag == "tr":
            self.row = []
        elif tag in ("td", "th") and self.row is not None:
            self.row.append("")

    def handle_endtag(self, tag):
        if tag in ("style", "script"):
            self.skip -= 1
            return
        if not self.in_body or self.skip:
            return
        if tag == "b":
            self.text("**: " if self.label else "**")
            self.label = False
        elif tag == "code":
            self.text("`")
        elif tag == "a" and self.href is not None:
            href, label = self.href, " ".join(self.link_text.split())
            self.href = None
            self.text(f"[{label}]({href})")
        elif tag == "tr" and self.row is not None:
            self.rows.append([" ".join(c.split()) for c in self.row])
            self.row = None
        elif tag == "table":
            head, *body = self.rows
            lines = ["| " + " | ".join(head) + " |", "|" + "---|" * len(head)]
            lines += ["| " + " | ".join(r) + " |" for r in body]
            self.out.append("\n".join(lines))
            self.rows = []
        elif tag in BLOCKS or tag == "li":
            self.flush()
        if tag in ("ul", "ol") and self.lists:
            self.lists.pop()

    def handle_data(self, data):
        if self.in_body and not self.skip:
            self.text(data)


def convert(html):
    p = ToMarkdown()
    p.feed(html)
    p.flush()
    md, blocks = [], p.out
    for i, b in enumerate(blocks):
        # keep list items together, blank line between other blocks
        item = re.compile(r"(- |\d+\. )")
        tight = i and item.match(b) and item.match(blocks[i - 1])
        md.append(("\n" if tight else "\n\n") + b if i else b)
    return "".join(md).rstrip() + "\n"


def main():
    docs = Path(sys.argv[1] if len(sys.argv) > 1 else "docs")
    md = convert((docs / "index.html").read_text(encoding="utf-8"))
    (docs / "index.md").write_text(md, encoding="utf-8")
    (docs / "llms-full.txt").write_text(md, encoding="utf-8")
    print(md)


if __name__ == "__main__":
    main()
