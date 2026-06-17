import DefaultTheme from 'vitepress/theme'
import type { Theme } from 'vitepress'
import Landing from './Landing.vue'
import './style.css'

export default {
  extends: DefaultTheme,
  // Render the custom landing for pages with `layout: landing`, otherwise the
  // default doc layout.
  Layout: Landing,
} satisfies Theme
