// Lays out what GitHub Pages serves at https://docs.bijectivelabs.dev: the platform's site under
// /bijective/, a root that leads there, and a forwarding page at every address the TRA
// documentation used to have on this domain, so links and search results keep working.
// Usage: node docs/.vitepress/site-root.mjs <built-site> <out-dir>
import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const [built, out] = process.argv.slice(2)
if (!built || !out || !existsSync(join(built, 'index.html'))) {
  console.error('usage: node site-root.mjs <built-site> <out-dir>')
  process.exit(2)
}
const here = dirname(fileURLToPath(import.meta.url))
const moved = JSON.parse(readFileSync(join(here, 'moved.json'), 'utf-8'))

// A page that says where its content went, read the same way by a browser and a crawler.
function forward(to) {
  return [
    '<!doctype html>',
    '<html lang="en"><head><meta charset="utf-8">',
    `<title>Moved to ${to}</title>`,
    `<link rel="canonical" href="${to}">`,
    '<meta name="robots" content="noindex">',
    `<meta http-equiv="refresh" content="0; url=${to}">`,
    `</head><body><p>This page moved to <a href="${to}">${to}</a>.</p></body></html>`,
    ''
  ].join('\n')
}

mkdirSync(out, { recursive: true })
cpSync(built, join(out, 'bijective'), { recursive: true })
cpSync(join(built, '404.html'), join(out, '404.html'))
writeFileSync(join(out, 'index.html'), forward('https://docs.bijectivelabs.dev/bijective/'))
writeFileSync(
  join(out, 'robots.txt'),
  'User-agent: *\nAllow: /\n\nSitemap: https://docs.bijectivelabs.dev/bijective/sitemap.xml\n'
)

let pages = 0
for (const [path, to] of Object.entries(moved.pages)) {
  const file = path.endsWith('/') ? join(out, path, 'index.html') : join(out, `${path}.html`)
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, forward(to))
  pages += 1
}
console.log(`site laid out in ${out}: /bijective/, the root, and ${pages} forwarding page(s)`)
