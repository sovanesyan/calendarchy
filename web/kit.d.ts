// window.Kit's types (kit/web/theme.js), for the pages' `tsc` check: no build, nothing
// served. Each page's jsconfig.json includes this file, so `Kit.…` is checked against it,
// and theme.js's own `root.Kit = {…}` is too: a helper added there and not here (or the
// other way round) fails the check. kit/sync.sh copies it next to each copy of theme.js.
//
// The kit's (~/Work/rb/ghostwheel/kit/web/kit.d.ts): change it there, not in a copy.

interface KitRgb { r: number; g: number; b: number }

/** /api/theme's answer: the Omarchy theme's colors.toml keys */
interface KitTheme {
  background?: string; foreground?: string; muted?: string; accent?: string
  red?: string; green?: string; yellow?: string
  dark_foreground?: string; light_foreground?: string; mode?: string
  [key: string]: unknown
}

/** The tokens for a theme: --bg, --fg, --good, … and whether it's dark */
interface KitThemeVars { dark: boolean; vars: Record<string, string> }

/** Kit.h's children: text (never HTML), nodes, nested lists; null/undefined/false are skipped */
type KitKid = Node | string | number | null | undefined | false | KitKid[]

/** Kit.h's attributes: a function is a listener (onclick), true an empty attribute, null/false none */
type KitAttrs = Record<string, unknown> | null | undefined

type KitPushState = "unsupported" | "install" | "denied" | "off" | "on"

interface KitSwipeAction { label: string; color?: string; run(row: HTMLElement): void }
interface KitFlingAction { label: string; color?: string; run(card: HTMLElement): void }
interface KitSheetAction { label: string; danger?: boolean; run?(): unknown }

interface KitApi {
  pullToRefresh(onRefresh: () => unknown): HTMLElement | null
  pullTravel(dy: number): number
  PULL_AT: number

  loadTheme(url?: string): Promise<{ theme: KitTheme; dark: boolean } | null>
  applyTheme(t: KitTheme | null | undefined): KitThemeVars | null
  themeVars(t: KitTheme | null | undefined): KitThemeVars | null
  goodColor(green: string | null | undefined, bgHex: string | null | undefined): string
  warnColor(yellow: string | null | undefined, bgHex: string | null | undefined): string
  urgentColor(red: string | null | undefined, bgHex: string | null | undefined): string
  onAccent(accentHex: string | null | undefined): string
  contrastOf(aHex: string, bHex: string): number
  readable(colorHex: string, bgHex: string, fgHex: string, min: number): string

  rgbOf(x: unknown): KitRgb | null
  hex(c: KitRgb): string
  mix(a: KitRgb, b: KitRgb, p: number): KitRgb
  luminance(c: KitRgb): number
  contrast(a: KitRgb, b: KitRgb): number
  isGreen(c: KitRgb): boolean
  isAmber(c: KitRgb): boolean

  h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs?: KitAttrs, kids?: KitKid): HTMLElementTagNameMap[K]
  h(tag: string, attrs?: KitAttrs, kids?: KitKid): HTMLElement

  offline(onChange?: (offline: boolean, saved: number | null) => void): Promise<ServiceWorkerRegistration | null>
  fmtSaved(ms: number | null | undefined, now?: number): string
  autoUpdate(opts?: { every?: number }): boolean
  pageChanged(have: string | null | undefined, now: string | null | undefined, ok: boolean, reloadedFor: string | null | undefined): boolean
  push: {
    state(): Promise<KitPushState>
    enable(): Promise<KitPushState>
    disable(): Promise<KitPushState>
    refresh(): Promise<boolean>
  }
  pushButton(): HTMLButtonElement
  badge(n: number | null | undefined): Promise<boolean>
  tick(): void
  share(data: ShareData): Promise<"shared" | "copied" | "cancelled">
  transition(update: () => void): Promise<void>
  keyboard(): void
  swipe<T extends HTMLElement>(row: T, opts: { left?: KitSwipeAction; right?: KitSwipeAction }): T
  swipeTravel(dx: number, width: number): number
  SWIPE_AT: number
  fling(card: HTMLElement, opts: { left?: KitFlingAction; right?: KitFlingAction }): { go(side: "left" | "right"): void }
  flingTilt(dx: number, width: number): number
  FLING_AT: number
  choice(o: { options: { value: string; label: string; hint?: string }[]; value: string; onChange?(value: string): void }): { el: HTMLElement; set(value: string): void; readonly value: string }
  autogrow(ta: HTMLTextAreaElement, max?: number): () => void
  sheet(o: { title?: string; text?: string; actions?: KitSheetAction[] }): Promise<string | null>
  privacy(): void
  lock(opts?: { app?: string; after?: number }): Promise<boolean>
  standalone(): boolean
}

interface Window { Kit: KitApi }
/** iOS Safari: whether the page runs as the home-screen app */
interface Navigator { standalone?: boolean }
declare var Kit: KitApi
