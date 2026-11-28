# Documentation

The platform's documentation, starting at [index.md](index.md). The same pages are published as
the site at https://docs.bijectivelabs.dev/bijective/, built with VitePress by
`.github/workflows/docs.yml`, which also lays out the domain root and forwards the addresses the TRA
documentation had on it (). A page joins the site's sidebar by being listed in its section's
`index.md`, and a change that breaks the build fails in CI like any other.

```bash
cd docs
npm ci
npm run dev      # the site at http://localhost:5173/bijective/
npm run build    # what CI publishes, in .vitepress/dist
```
