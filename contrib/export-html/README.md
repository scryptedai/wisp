# HTML export smoke (`wisp export --html`)

Part 1 offline site export. Spec: [docs/EXPORT-HTML.md](../../docs/EXPORT-HTML.md).

## Generate a site

```sh
wisp export --html /tmp/wisp-site --ref main
open /tmp/wisp-site/index.html
```

From the `impact-demo` fixture in CI:

```sh
WRITE_HTML_DEMO=1 cargo test -p wisp-invariants --test export_html export_html_impact_demo -- --nocapture
open /tmp/wisp-site-demo/index.html
```

## Five-viewer smoke (60s)

Open `index.html`, then the **webhook-setup** concept page and a section (fragment `#…` or scroll). Each viewer should answer within one minute:

1. Who authored the current version? (actor line)
2. What input did they declare? (inputs list / pin state)
3. Is there a retraction or erasure marker on the overview?
4. Does a merge-skipped version show “Not kept at merge …”? (warehouse merge fixture in tests)
5. Does the yellow plaintext-copy banner appear on every page?

Pass: **4/5** correct without installing wisp.
