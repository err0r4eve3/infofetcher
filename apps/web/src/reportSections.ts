import { Bitcoin, Building2, ChartNoAxesCombined, Cpu, Flame, FlaskConical, Landmark, Layers3, Palette, ShieldCheck, Sparkles, Swords, Users, type LucideIcon } from 'lucide-react'

// 与后端 world.rs 的 REPORT_SECTIONS 同序同键。
export const REPORT_SECTIONS = [
  { key: 'conflict', label: '战争与冲突', icon: Swords },
  { key: 'politics', label: '政治与外交', icon: Landmark },
  { key: 'society', label: '社会与民生', icon: Users },
  { key: 'disaster', label: '灾害与事故', icon: Flame },
  { key: 'economy', label: '宏观与市场', icon: ChartNoAxesCombined },
  { key: 'business', label: '公司与产业', icon: Building2 },
  { key: 'ai', label: 'AI', icon: Sparkles },
  { key: 'tech', label: '科技与产品', icon: Cpu },
  { key: 'cyber', label: '网络安全', icon: ShieldCheck },
  { key: 'science', label: '科学与健康', icon: FlaskConical },
  { key: 'crypto', label: '加密资产', icon: Bitcoin },
  { key: 'culture', label: '文化与体育', icon: Palette },
  { key: 'general', label: '其他', icon: Layers3 }
] as const satisfies ReadonlyArray<{ key: string; label: string; icon: LucideIcon }>

export type ReportSectionKey = typeof REPORT_SECTIONS[number]['key']

// 2026-10-20 栏目细分前保存的事件仍带旧键。
const LEGACY_SECTIONS: Record<string, ReportSectionKey> = { world: 'politics', technology: 'tech', security: 'cyber' }

export function normalizeReportSection(value: string): ReportSectionKey {
  const key = LEGACY_SECTIONS[value] ?? value
  return REPORT_SECTIONS.some((section) => section.key === key) ? key as ReportSectionKey : 'general'
}
