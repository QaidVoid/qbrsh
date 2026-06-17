<script setup lang="ts">
import { useData } from 'vitepress'
import DefaultTheme from 'vitepress/theme'

const { Layout } = DefaultTheme
const { frontmatter } = useData()

// The core normal-mode bindings, shown as keycaps on the landing page.
const keys = [
  { cap: 'f', desc: 'follow a link' },
  { cap: 'o', desc: 'open a URL' },
  { cap: 't', desc: 'open in a tab' },
  { cap: 'hjkl', desc: 'scroll', wide: true },
  { cap: 'gg', desc: 'top of page' },
  { cap: 'yy', desc: 'yank URL' },
  { cap: 'b', desc: 'load quickmark' },
  { cap: ':', desc: 'command line' },
]

const features = [
  {
    title: 'Hint mode',
    body: 'Press f and every link gets a label. Type it to follow. No mouse required.',
  },
  {
    title: 'Modal and fast',
    body: 'A hand-rolled Elm-style core on WebKitGTK. One owned state, no re-entrancy, no polling.',
  },
  {
    title: 'Rune plugins',
    body: 'Sandboxed async scripts with cold-event hooks and an instruction budget. Reload with :plugin-reload.',
  },
  {
    title: 'Native ad blocking',
    body: 'Domain blocking at navigation plus a WebKit content filter for subresources. No extension needed.',
  },
  {
    title: 'Fuzzy completion',
    body: 'Cycle the command line with Tab, backed by history, bookmarks, and quickmarks.',
  },
  {
    title: 'Scriptable',
    body: 'Drive the browser from any process over a JSON-RPC control socket.',
  },
]
</script>

<template>
  <div v-if="frontmatter.layout === 'landing'" class="qb-landing">
    <header class="qb-hero">
      <div class="qb-statusbar">
        <span class="qb-mode">NORMAL</span>
        <span class="qb-url">qbrsh://welcome</span>
        <span class="qb-scroll">100%</span>
        <span class="qb-tabs">[1/1]</span>
      </div>

      <h1 class="qb-wordmark">qbrsh</h1>
      <p class="qb-tagline">A fast, keyboard-driven web browser in Rust.</p>
      <p class="qb-sub">
        Vim-style navigation on WebKitGTK, with hint-mode link following, fuzzy
        completion, native ad blocking, and a sandboxed Rune plugin runtime.
      </p>

      <div class="qb-cta">
        <a class="qb-btn qb-btn-primary" href="/guide/getting-started">Get started</a>
        <a class="qb-btn" href="https://github.com/QaidVoid/qbrsh">View source</a>
      </div>

      <div class="qb-keys">
        <div v-for="k in keys" :key="k.cap" class="qb-key" :class="{ wide: k.wide }">
          <kbd>{{ k.cap }}</kbd>
          <span>{{ k.desc }}</span>
        </div>
      </div>
    </header>

    <section class="qb-features">
      <article v-for="f in features" :key="f.title" class="qb-card">
        <h3>{{ f.title }}</h3>
        <p>{{ f.body }}</p>
      </article>
    </section>

    <footer class="qb-foot">
      <span>Built on the TEA core.</span>
      <a href="/guide/architecture">How it works</a>
    </footer>
  </div>

  <Layout v-else />
</template>
