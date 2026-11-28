// The documentation site, built from this directory and published at
// https://docs.bijectivelabs.dev/bijective/. The pages are the repository's own Markdown, so the
// site changes in the same commit as the source it describes.
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, posix, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitepress'

const repo = 'https://github.com/BijectiveLabs/bijective'
const docsRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const repoRoot = resolve(docsRoot, '..')

// The sidebar is the sections docs/index.md lists, each holding the pages its own index lists, in
// that order. A page joins the site by being listed, as it joins the documentation.
function linksIn(file: string): { text: string; path: string }[] {
  return [...readFileSync(file, 'utf-8').matchAll(/\[([^\]]+)\]\(([^)#\s]+\.md)\)/g)].map((m) => ({
    text: m[1],
    path: posix.normalize(m[2])
  }))
}

function route(path: string): string {
  return '/' + path.replace(/\.md$/, '').replace(/(^|\/)index$/, '$1')
}

function sidebar() {
  const groups = [{ text: 'Bijective', items: [{ text: 'Overview', link: '/' }] }]
  for (const section of linksIn(join(docsRoot, 'index.md'))) {
    const index = join(docsRoot, section.path)
    if (!existsSync(index)) continue
    const base = posix.dirname(section.path)
    const items = [{ text: 'Introduction', link: route(section.path) }]
    for (const page of linksIn(index)) {
      const path = posix.normalize(posix.join(base, page.path))
      if (!path.startsWith('..') && existsSync(join(docsRoot, path))) {
        items.push({ text: page.text, link: route(path) })
      }
    }
    groups.push({ text: section.text, collapsed: true, items } as never)
  }
  return groups
}

// A link that leaves docs/ (the README, a crate, a script) points at the file on GitHub, where it
// lives. Links inside docs/ stay relative and are checked by the build.
function outsideLinks(md: { core: { ruler: { push: (name: string, rule: (state: never) => void) => void } } }) {
  md.core.ruler.push('bijective-outside-links', (state: { env: { relativePath?: string }; tokens: never[] }) => {
    const page = state.env.relativePath
    if (!page) return
    const walk = (tokens: { type: string; attrGet: (n: string) => string | null; attrSet: (n: string, v: string) => void; children?: unknown[] }[]) => {
      for (const token of tokens) {
        if (token.children) walk(token.children as never)
        if (token.type !== 'link_open') continue
        const href = token.attrGet('href')
        if (!href || /^[a-z]+:|^#|^\//i.test(href)) continue
        const [path, anchor] = href.split('#')
        const target = resolve(docsRoot, dirname(page), path)
        if (target.startsWith(docsRoot + '/')) continue
        const inRepo = relative(repoRoot, target).split('\\').join('/')
        const kind = path.endsWith('/') ? 'tree' : 'blob'
        // master, as the site is: the published pages describe the latest release.
        token.attrSet('href', `${repo}/${kind}/master/${inRepo}${anchor ? `#${anchor}` : ''}`)
      }
    }
    walk(state.tokens as never)
  })
}

export default defineConfig({
  lang: 'en-US',
  title: 'Bijective',
  titleTemplate: ':title | Bijective platform',
  description:
    'Documentation for Bijective, a self-hosted multi-asset trading platform: every order passes a ' +
    'mandatory risk gate and every decision is replayable from an append-only record.',
  base: '/bijective/',
  appearance: 'dark',
  cleanUrls: true,
  lastUpdated: true,
  srcExclude: ['**/node_modules/**', 'README.md'],
  sitemap: { hostname: 'https://docs.bijectivelabs.dev/bijective/' },
  head: [
    ['link', { rel: 'icon', href: '/bijective/icon.svg' }],
    ['link', { rel: 'preconnect', href: 'https://fonts.googleapis.com' }],
    ['link', { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&family=Montserrat:wght@100;300;400&display=swap' }]
  ],
  markdown: { config: outsideLinks as never },
  themeConfig: {
    logo: { light: '/bijective_docs_logo_black.svg', dark: '/bijective_docs_logo.svg', alt: 'BIJECTIVE' },
    siteTitle: false,
    search: { provider: 'local' },
    socialLinks: [{ icon: 'github', link: repo }],
    nav: [
      { text: 'Platform', link: '/' },
      { text: 'Releases', link: `${repo}/releases` },
      { text: 'Bijective Labs', link: 'https://www.bijectivelabs.dev/bijective' }
    ],
    sidebar: sidebar(),
    editLink: { pattern: `${repo}/edit/dev/docs/:path`, text: 'Edit this page on GitHub' },
    outline: { level: [2, 3], label: 'On this page' },
    footer: {
      message: 'Released under the Apache License 2.0.',
      copyright: 'Copyright 2026 Bijective Labs'
    }
  }
})
