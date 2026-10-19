import { filterSources, type SourceStatus } from './sourceFilters'
import './sourceFilters.css'
import { shortSourceName } from './coverageModel'
import {
  ArrowDown,
  ArrowLeft,
  ArrowUp,
  BellRing,
  CheckCircle2,
  Circle,
  CircleDot,
  Download,
  FileUp,
  FolderPlus,
  Import,
  Languages,
  ListChecks,
  LogOut,
  Moon,
  Pencil,
  Plus,
  Radar,
  RefreshCw,
  Rss,
  Search,
  Send,
  Settings,
  Sparkles,
  Star,
  Sun,
  Tags,
  Trash2,
  VolumeX
} from 'lucide-react'
import {
  type CSSProperties,
  type ChangeEvent,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type RefObject,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState
} from 'react'
import {
  api,
  ApiError,
  ArticleDetail,
  ArticleListItem,
  ArticleTranslation,
  BriefEditionList,
  EditorialProfile,
  Feed,
  Folder,
  formatDate,
  KeywordActionResponse,
  KeywordMatch,
  KeywordRule,
  KeywordRuleRequest,
  KeywordScanResponse,
  Metrics,
  Monitor,
  MonitorRequest,
  prefetch,
  StoryCluster,
  Summary
} from './api'
import { formatArticleParagraphs } from './articleText'
import { SelectionTranslator } from './SelectionTranslator'
import { WorldRadar } from './WorldRadar'

// The radar is the default view: request its first report alongside the session check instead of
// after it. An unauthenticated prefetch simply fails and the radar requests again.
if (readAppView() === 'now') prefetch('/api/v2/brief/now?hours=12')

type User = { username: string }

type Theme = 'light' | 'dark'
type AppView = 'now' | 'reader' | 'settings'
type NavigationMode = 'push' | 'replace'

type AppNavigationOptions = {
  mode?: NavigationMode
  query?: string
  articleId?: string | null
}

function readAppView(): AppView {
  if (typeof window === 'undefined') return 'now'
  const value = new URLSearchParams(window.location.search).get('view')
  return value === 'reader' || value === 'settings' ? value : 'now'
}

function readLibraryQuery(): string {
  if (typeof window === 'undefined') return ''
  return new URLSearchParams(window.location.search).get('q') ?? ''
}

function readArticleId(): string | null {
  if (typeof window === 'undefined') return null
  return new URLSearchParams(window.location.search).get('article')
}

function writeAppLocation(
  view: AppView,
  { mode = 'push', query = '', articleId = null }: AppNavigationOptions = {}
) {
  const url = new URL(window.location.href)
  if (view === 'now') url.searchParams.delete('view')
  else url.searchParams.set('view', view)
  if (view === 'reader' && query) url.searchParams.set('q', query)
  else url.searchParams.delete('q')
  if (view === 'reader' && articleId) url.searchParams.set('article', articleId)
  else url.searchParams.delete('article')
  window.history[mode === 'push' ? 'pushState' : 'replaceState'](
    { ...window.history.state, appView: view, query, articleId },
    '',
    url
  )
}

type ReadyMadeFeed = {
  title: string
  url?: string
  rsshubPath?: string
  category: string
  qualityScore?: number
}

type FeedPreferencePatch = {
  feedback_weight?: number
  muted?: boolean
  editorial_profile?: EditorialProfile | 'auto'
}

const EDITORIAL_PROFILE_LABELS: Record<EditorialProfile, string> = {
  'world-news': '时事要闻',
  'tech-research': '科技与研究',
  'long-read': '深度长文'
}

type FeedGroup = {
  folder: Folder
  feeds: Feed[]
  unreadCount: number
}

type PresetGroup = {
  category: string
  feeds: ReadyMadeFeed[]
}

type SocialSourceAdapter = 'activitypub' | 'rsshub' | 'opencli'

type OpenCliSocialSource = {
  name: string
  title: string
  link: string | null
  description: string | null
}

type RssHubSocialSource = {
  title: string
  path: string
  description: string
}

type SocialSourceCatalog = {
  activitypub_enabled: boolean
  rsshub_enabled: boolean
  opencli: OpenCliSocialSource[]
}

type ReaderLayout = {
  sidebar: number
  list: number
}

type ResizeTarget = 'sidebar' | 'list'
type ContentView = 'today' | 'all' | 'monitor'
type SortMode = 'priority' | 'latest'
type MobilePane = 'sources' | 'articles' | 'reader'

const READER_LAYOUT_STORAGE_KEY = 'infofetcher-reader-layout-v2'
const THEME_STORAGE_KEY = 'infofetcher-theme'
const LEGACY_READER_LAYOUT_STORAGE_KEY = 'rss-reader-layout-v2'
const LEGACY_THEME_STORAGE_KEY = 'rss-hub-reader-theme'
const DEFAULT_READER_LAYOUT: ReaderLayout = { sidebar: 320, list: 320 }
const RESIZER_WIDTH = 8
const READER_SHELL_INLINE_PADDING = 20
const MIN_SIDEBAR_WIDTH = 220
const MAX_SIDEBAR_WIDTH = 420
const MIN_ARTICLE_LIST_WIDTH = 280
const MAX_ARTICLE_LIST_WIDTH = 640
const MIN_READER_WIDTH = 500
const ARTICLE_PAGE_SIZE = 50
const SUMMARY_POLL_INTERVAL_MS = 2_000
const SUMMARY_POLL_DEADLINE_MS = 120_000
const SUMMARY_REQUEST_TIMEOUT_MS = 10_000
const PRESET_CATEGORY_ORDER = [
  '一手发布',
  '老牌媒体',
  '科技媒体',
  '科技',
  '工程博客',
  '专家观点',
  'AI 研究',
  '信息安全',
  '论坛'
]

const TELEGRAM_NEWS_SOURCES: RssHubSocialSource[] = [
  { title: '科技圈 · 在花频道', path: 'telegram/channel/testflightcn', description: '@testflightcn' },
  { title: '竹新社', path: 'telegram/channel/tnews365', description: '@tnews365' },
  { title: 'LoopDNS 资讯播报', path: 'telegram/channel/DNSPODT', description: '@DNSPODT' },
  {
    title: 'Newlearner の自留地',
    path: 'telegram/channel/NewlearnerChannel',
    description: '@NewlearnerChannel'
  },
  {
    title: 'Breaking Alert 全球快报',
    path: 'telegram/channel/GlobalFinance_ZH',
    description: '@GlobalFinance_ZH'
  }
]

function waitWithAbort(milliseconds: number, signal: AbortSignal): Promise<boolean> {
  return new Promise((resolve) => {
    if (signal.aborted) {
      resolve(false)
      return
    }
    const timer = window.setTimeout(() => {
      signal.removeEventListener('abort', onAbort)
      resolve(true)
    }, milliseconds)
    function onAbort() {
      window.clearTimeout(timer)
      resolve(false)
    }
    signal.addEventListener('abort', onAbort, { once: true })
  })
}

const READY_MADE_FEEDS: ReadyMadeFeed[] = [
  { title: 'NASA News Releases', url: 'https://www.nasa.gov/news-release/feed/', category: '一手发布' },
  { title: 'USGS Significant Earthquakes', url: 'https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/significant_month.atom', category: '一手发布' },
  { title: 'CISA Cybersecurity Advisories', url: 'https://www.cisa.gov/cybersecurity-advisories/all.xml', category: '信息安全', qualityScore: 84 },
  { title: 'UN News', url: 'https://news.un.org/feed/subscribe/en/news/all/rss.xml', category: '一手发布' },
  { title: 'Federal Reserve Monetary Policy', url: 'https://www.federalreserve.gov/feeds/press_monetary.xml', category: '一手发布' },
  { title: 'ECB Press Releases', url: 'https://www.ecb.europa.eu/rss/press.html', category: '一手发布' },
  { title: '国家统计局 最新发布', url: 'https://www.stats.gov.cn/sj/zxfb/rss.xml', category: '一手发布' },
  { title: '国家统计局 数据解读', url: 'https://www.stats.gov.cn/sj/sjjd/rss.xml', category: '一手发布' },
  { title: '工信部 政策解读', rsshubPath: 'gov/miit/zcjd', category: '一手发布' },
  { title: '国家网信办 信息化', rsshubPath: 'gov/cac/wxzw/xxh', category: '一手发布' },
  { title: 'NodeSeek', url: 'https://rss.nodeseek.com/', category: '论坛' },
  { title: 'LINUX DO 最新', url: 'https://linux.do/latest.rss?order=created', category: '论坛' },
  { title: 'V2EX 全站最新', url: 'https://www.v2ex.com/index.xml', category: '论坛' },
  { title: 'V2EX 技术', url: 'https://www.v2ex.com/feed/tab/tech.xml', category: '论坛' },
  { title: 'V2EX 程序员', url: 'https://www.v2ex.com/feed/programmer.xml', category: '论坛' },
  { title: 'V2EX RSS 节点', url: 'https://www.v2ex.com/feed/rss.xml', category: '论坛' },
  { title: 'BBC News', url: 'https://feeds.bbci.co.uk/news/rss.xml', category: '老牌媒体' },
  { title: 'BBC World', url: 'https://feeds.bbci.co.uk/news/world/rss.xml', category: '老牌媒体' },
  { title: 'The Guardian World', url: 'https://www.theguardian.com/world/rss', category: '老牌媒体' },
  { title: 'The New York Times', url: 'https://rss.nytimes.com/services/xml/rss/nyt/HomePage.xml', category: '老牌媒体' },
  { title: 'The Washington Post World', url: 'https://feeds.washingtonpost.com/rss/world', category: '老牌媒体' },
  { title: 'Financial Times', url: 'https://www.ft.com/rss/home', category: '老牌媒体' },
  { title: 'WSJ World News', url: 'https://feeds.a.dj.com/rss/RSSWorldNews.xml', category: '老牌媒体' },
  { title: 'NPR News', url: 'https://feeds.npr.org/1001/rss.xml', category: '老牌媒体' },
  { title: 'Le Monde International', url: 'https://www.lemonde.fr/en/international/rss_full.xml', category: '老牌媒体' },
  { title: 'Al Jazeera English', url: 'https://www.aljazeera.com/xml/rss/all.xml', category: '老牌媒体' },
  { title: 'France 24', url: 'https://www.france24.com/en/rss', category: '老牌媒体' },
  { title: 'cnBeta 中文科技', url: 'https://www.cnbeta.com.tw/backend.php', category: '科技媒体' },
  { title: 'MacRumors', url: 'https://feeds.macrumors.com/MacRumors-All', category: '科技媒体' },
  { title: 'Phoronix', url: 'https://www.phoronix.com/rss.php', category: '科技媒体' },
  { title: 'Ars Technica', url: 'https://feeds.arstechnica.com/arstechnica/index', category: '科技媒体' },
  { title: 'TechCrunch', url: 'https://techcrunch.com/feed/', category: '科技媒体' },
  { title: 'The Verge', url: 'https://www.theverge.com/rss/index.xml', category: '科技媒体' },
  { title: '9to5Mac', url: 'https://9to5mac.com/feed/', category: '科技媒体' },
  { title: 'BleepingComputer', url: 'https://www.bleepingcomputer.com/feed/', category: '科技媒体' },
  { title: 'The Register', url: 'https://www.theregister.com/headlines.atom', category: '科技媒体' },
  { title: 'LWN.net', url: 'https://lwn.net/headlines/rss', category: '科技媒体' },
  { title: 'Hacker News Front Page', url: 'https://hnrss.org/frontpage', category: '科技' },
  { title: 'GitHub Blog', url: 'https://github.blog/feed/', category: '科技' },
  { title: 'Rust Blog', url: 'https://blog.rust-lang.org/feed.xml', category: '科技' },
  { title: '阮一峰的网络日志', url: 'http://www.ruanyifeng.com/blog/atom.xml', category: '科技' },
  { title: 'AWS News Blog', url: 'https://aws.amazon.com/blogs/aws/feed/', category: '工程博客' },
  { title: 'Netflix TechBlog', url: 'https://netflixtechblog.com/feed', category: '工程博客' },
  { title: 'Cloudflare Blog', url: 'https://blog.cloudflare.com/rss/', category: '工程博客' },
  {
    title: 'Simon Willison',
    url: 'https://simonwillison.net/atom/entries/',
    category: '专家观点'
  },
  { title: 'One Useful Thing', url: 'https://www.oneusefulthing.org/feed', category: '专家观点' },
  { title: 'Import AI', url: 'https://importai.substack.com/feed', category: '专家观点' },
  {
    title: 'The Pragmatic Engineer',
    url: 'https://newsletter.pragmaticengineer.com/feed',
    category: '专家观点'
  },
  { title: 'Latent Space', url: 'https://www.latent.space/feed', category: '专家观点' },
  {
    title: 'SemiAnalysis',
    url: 'https://newsletter.semianalysis.com/feed',
    category: '专家观点'
  },
  { title: 'ChinaTalk', url: 'https://www.chinatalk.media/feed', category: '专家观点' },
  {
    title: 'Asianometry',
    url: 'https://www.youtube.com/feeds/videos.xml?channel_id=UC1LpsuAUaKoMzzJSEt5WImw',
    category: '专家观点'
  },
  { title: '硅谷101', url: 'https://feeds.fireside.fm/sv101/rss', category: '专家观点' },
  { title: 'OpenAI News', url: 'https://openai.com/news/rss.xml', category: 'AI 研究' },
  { title: 'Anthropic News', rsshubPath: 'anthropic/news', category: 'AI 研究' },
  { title: 'Hugging Face Weekly Papers', rsshubPath: 'huggingface/daily-papers/week/50', category: 'AI 研究', qualityScore: 72 },
  { title: 'Google Research Blog', url: 'https://research.google/blog/rss/', category: 'AI 研究' },
  { title: 'Google DeepMind', url: 'https://deepmind.google/blog/rss.xml', category: 'AI 研究' },
  { title: 'Hugging Face Blog', url: 'https://huggingface.co/blog/feed.xml', category: 'AI 研究' },
  {
    title: 'Microsoft Research',
    url: 'https://www.microsoft.com/en-us/research/feed/',
    category: 'AI 研究'
  },
  { title: 'arXiv cs.CL', url: 'https://rss.arxiv.org/rss/cs.CL', category: 'AI 研究' },
  { title: 'arXiv cs.LG', url: 'https://rss.arxiv.org/rss/cs.LG', category: 'AI 研究' },
  { title: 'Google Online Security Blog', url: 'https://security.googleblog.com/feeds/posts/default', category: '信息安全' },
  { title: 'Google Project Zero', url: 'https://googleprojectzero.blogspot.com/feeds/posts/default', category: '信息安全' },
  { title: 'GitHub Advisories · npm', rsshubPath: 'github/advisor/data/reviewed/npm', category: '信息安全', qualityScore: 84 },
  { title: 'GitHub Advisories · Rust', rsshubPath: 'github/advisor/data/reviewed/rust', category: '信息安全', qualityScore: 84 },
  { title: 'The Hacker News', url: 'https://feeds.feedburner.com/TheHackersNews', category: '信息安全' }
]

function presetSourceMetadata(preset: ReadyMadeFeed) {
  const isChinese = preset.category === '论坛' || /[\u3400-\u9fff]/.test(preset.title)
  const qualityByCategory: Record<string, number> = {
    '一手发布': 82,
    '老牌媒体': 85,
    '科技媒体': 80,
    '专家观点': 84,
    'AI 研究': 82,
    '工程博客': 80,
    '信息安全': 78,
    '科技': 72,
    '论坛': 58
  }
  return {
    source_type: preset.rsshubPath || preset.url?.includes('/hub/') ? 'adapter' : 'rss',
    language: isChinese ? 'zh' : 'en',
    region: isChinese ? 'CN' : 'GLOBAL',
    quality_score: preset.qualityScore ?? qualityByCategory[preset.category] ?? 60
  }
}

function matchesPresetFeed(feed: Feed, preset: ReadyMadeFeed) {
  return preset.rsshubPath
    ? feed.hub_adapter === 'rsshub' && feed.hub_path === preset.rsshubPath
    : feed.feed_url === preset.url
}

function presetDisplayUrl(preset: ReadyMadeFeed) {
  return preset.rsshubPath ? `RSSHub /${preset.rsshubPath}` : preset.url ?? ''
}

function priorityReasonLabel(reason: string): string {
  const labels: Record<string, string> = {
    multi_source: '多源确认',
    source_quality: '优质来源',
    fresh: '新近发布',
    favorite: '已收藏',
    user_preference: '偏好来源',
    detailed: '信息完整'
  }
  return labels[reason] ?? reason
}

function readInitialTheme(): Theme {
  if (typeof window === 'undefined') return 'light'
  try {
    const storedTheme = window.localStorage.getItem(THEME_STORAGE_KEY)
      ?? window.localStorage.getItem(LEGACY_THEME_STORAGE_KEY)
    if (storedTheme === 'light' || storedTheme === 'dark') return storedTheme
  } catch {
    // Ignore inaccessible storage and fall back to the system preference.
  }
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), Math.max(min, max))
}

function clampReaderLayout(layout: ReaderLayout, shellWidth: number): ReaderLayout {
  const available = Math.max(0, shellWidth - RESIZER_WIDTH * 2 - READER_SHELL_INLINE_PADDING)
  const sidebar = clamp(
    layout.sidebar,
    MIN_SIDEBAR_WIDTH,
    Math.min(MAX_SIDEBAR_WIDTH, available - MIN_ARTICLE_LIST_WIDTH - MIN_READER_WIDTH)
  )
  const list = clamp(
    layout.list,
    MIN_ARTICLE_LIST_WIDTH,
    Math.min(MAX_ARTICLE_LIST_WIDTH, available - sidebar - MIN_READER_WIDTH)
  )
  return { sidebar, list }
}

function resolveScrollBehavior(behavior: ScrollBehavior): ScrollBehavior {
  if (behavior !== 'smooth' || typeof window === 'undefined') return behavior
  return window.matchMedia?.('(prefers-reduced-motion: reduce)').matches ? 'auto' : behavior
}

function loadReaderLayout(): ReaderLayout {
  if (typeof window === 'undefined') return DEFAULT_READER_LAYOUT
  try {
    const stored = window.localStorage.getItem(READER_LAYOUT_STORAGE_KEY)
      ?? window.localStorage.getItem(LEGACY_READER_LAYOUT_STORAGE_KEY)
    if (!stored) return DEFAULT_READER_LAYOUT
    const parsed = JSON.parse(stored) as Partial<ReaderLayout>
    if (typeof parsed.sidebar !== 'number' || typeof parsed.list !== 'number') {
      return DEFAULT_READER_LAYOUT
    }
    if (window.matchMedia('(max-width: 820px)').matches) {
      return DEFAULT_READER_LAYOUT
    }
    return clampReaderLayout({ sidebar: parsed.sidebar, list: parsed.list }, window.innerWidth)
  } catch {
    return DEFAULT_READER_LAYOUT
  }
}

function resizeReaderLayout(layout: ReaderLayout, target: ResizeTarget, delta: number, shellWidth: number): ReaderLayout {
  if (target === 'sidebar') {
    return clampReaderLayout({ ...layout, sidebar: layout.sidebar + delta }, shellWidth)
  }
  return clampReaderLayout({ ...layout, list: layout.list + delta }, shellWidth)
}

// Hex of --app-bg per theme; keep in sync with styles.css.
const THEME_CHROME_COLOR: Record<Theme, string> = { light: '#f0f3f5', dark: '#1c1e21' }

export function App() {
  const [user, setUser] = useState<User | null | undefined>(undefined)
  const [error, setError] = useState<string | null>(null)
  const [theme, setTheme] = useState<Theme>(readInitialTheme)

  useLayoutEffect(() => {
    document.documentElement.dataset.theme = theme
    document.documentElement.style.colorScheme = theme
    // Browser chrome (mobile address bar, PWA title bar) follows the chosen theme, not only the OS one.
    document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]').forEach((meta) => {
      meta.removeAttribute('media')
      meta.content = THEME_CHROME_COLOR[theme]
    })
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, theme)
    } catch {
      // Theme still applies even when storage is unavailable.
    }
  }, [theme])

  const toggleTheme = useCallback(() => {
    setTheme((current) => (current === 'dark' ? 'light' : 'dark'))
  }, [])

  useEffect(() => {
    api<User>('/api/auth/me')
      .then(setUser)
      .catch(() => setUser(null))
  }, [])

  if (user === undefined) {
    return <div className="boot" role="status" aria-live="polite">正在连接阅读器…</div>
  }

  if (!user) {
    return (
      <LoginScreen
        onLogin={setUser}
        error={error}
        setError={setError}
        theme={theme}
        onToggleTheme={toggleTheme}
      />
    )
  }

  return (
    <ReaderApp
      user={user}
      onLogout={() => setUser(null)}
      theme={theme}
      onToggleTheme={toggleTheme}
    />
  )
}

function LoginScreen({
  onLogin,
  error,
  setError,
  theme,
  onToggleTheme
}: {
  onLogin: (user: User) => void
  error: string | null
  setError: (value: string | null) => void
  theme: Theme
  onToggleTheme: () => void
}) {
  const [token, setToken] = useState('')
  const [submitting, setSubmitting] = useState(false)

  async function submit(event: FormEvent) {
    event.preventDefault()
    setSubmitting(true)
    setError(null)
    try {
      const user = await api<User>('/api/auth/login', {
        method: 'POST',
        body: JSON.stringify({ token })
      })
      onLogin(user)
    } catch (err) {
      setError(err instanceof Error ? err.message : '登录失败')
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <main className="login-shell">
      <form className="login-panel" onSubmit={submit}>
        <div className="login-heading">
          <div className="brand-line">
            <Rss size={28} />
            <div>
              <h1>Infofetcher</h1>
            </div>
          </div>
          <ThemeToggle theme={theme} onToggleTheme={onToggleTheme} />
        </div>
        <label>
          Token
          <input
            value={token}
            onChange={(event) => setToken(event.target.value)}
            type="password"
            name="token"
            autoComplete="current-password"
            autoCapitalize="none"
            spellCheck={false}
            required
            maxLength={256}
          />
        </label>
        {error && <div className="error-line" role="alert">{error}</div>}
        <button className="primary-button" disabled={submitting || !token}>
          {submitting ? '登录中…' : '打开信息雷达'}
        </button>
      </form>
    </main>
  )
}

function ReaderApp({
  user,
  onLogout,
  theme,
  onToggleTheme
}: {
  user: User
  onLogout: () => void
  theme: Theme
  onToggleTheme: () => void
}) {
  const [activeView, setActiveView] = useState<AppView>(readAppView)
  const [feeds, setFeeds] = useState<Feed[]>([])
  const [folders, setFolders] = useState<Folder[]>([])
  const [articles, setArticles] = useState<ArticleListItem[]>([])
  const [contentView, setContentView] = useState<ContentView>('today')
  const [sortMode, setSortMode] = useState<SortMode>('priority')
  const [hasMoreArticles, setHasMoreArticles] = useState(false)
  const [monitors, setMonitors] = useState<Monitor[]>([])
  const [selectedMonitorId, setSelectedMonitorId] = useState<string | null>(null)
  const [selectedStory, setSelectedStory] = useState<StoryCluster | null>(null)
  const [mobilePane, setMobilePane] = useState<MobilePane>('articles')
  const [selectedFeedId, setSelectedFeedId] = useState<string | null>(null)
  const [selectedFolderId, setSelectedFolderId] = useState<string | null>(null)
  const [selectedArticleId, setSelectedArticleId] = useState<string | null>(readArticleId)
  const [selectedArticle, setSelectedArticle] = useState<ArticleDetail | null>(null)
  const [metrics, setMetrics] = useState<Metrics | null>(null)
  const [keywordRules, setKeywordRules] = useState<KeywordRule[]>([])
  const [keywordMatches, setKeywordMatches] = useState<KeywordMatch[]>([])
  const [keywordMatchRuleFilter, setKeywordMatchRuleFilter] = useState('')
  const [selectedKeywordMatchIds, setSelectedKeywordMatchIds] = useState<string[]>([])
  const [query, setQuery] = useState(readLibraryQuery)
  const [unreadOnly, setUnreadOnly] = useState(false)
  const [favoriteOnly, setFavoriteOnly] = useState(false)
  const [feedUrl, setFeedUrl] = useState('')
  const [newFeedFolderId, setNewFeedFolderId] = useState('')
  const [folderName, setFolderName] = useState('')
  const [opmlText, setOpmlText] = useState('')
  const [socialSourceCatalog, setSocialSourceCatalog] = useState<SocialSourceCatalog | null>(null)
  const [socialCatalogLoading, setSocialCatalogLoading] = useState(false)
  const [socialCatalogError, setSocialCatalogError] = useState<string | null>(null)
  const [activityPubHandle, setActivityPubHandle] = useState('')
  const [rssHubRoute, setRssHubRoute] = useState('')
  const [busy, setBusy] = useState(false)
  const [summarizingIds, setSummarizingIds] = useState<Set<string>>(() => new Set())
  const [translatingIds, setTranslatingIds] = useState<Set<string>>(() => new Set())
  const [showTranslation, setShowTranslation] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [readerLayout, setReaderLayout] = useState<ReaderLayout>(loadReaderLayout)
  const opmlFileInputRef = useRef<HTMLInputElement | null>(null)
  const articleRequestGenerationRef = useRef(0)
  const articleRequestControllerRef = useRef<AbortController | null>(null)
  const directArticleOpenRef = useRef(activeView === 'reader' && Boolean(selectedArticleId))
  const shellLoadedRef = useRef(false)
  const shellRequestRef = useRef<Promise<void> | null>(null)
  const summaryPollControllersRef = useRef(new Map<string, AbortController>())
  const selectedArticleIdRef = useRef<string | null>(selectedArticleId)
  selectedArticleIdRef.current = selectedArticleId

  const navigateToView = useCallback((nextView: AppView, options: AppNavigationOptions = {}) => {
    const nextQuery = options.query ?? (nextView === 'reader' ? query : '')
    const nextArticleId = options.articleId === undefined
      ? (nextView === 'reader' ? selectedArticleId : null)
      : options.articleId
    writeAppLocation(nextView, { ...options, query: nextQuery, articleId: nextArticleId })
    setActiveView(nextView)
  }, [query, selectedArticleId])

  const selectedFeed = feeds.find((feed) => feed.id === selectedFeedId) ?? null
  const selectedFolder = folders.find((folder) => folder.id === selectedFolderId) ?? null
  const selectedMonitor = monitors.find((monitor) => monitor.id === selectedMonitorId) ?? null
  const contextTitle = contentView === 'today'
    ? '今日重点'
    : contentView === 'monitor'
      ? selectedMonitor?.name ?? '主题监控'
      : selectedFeed?.title ?? selectedFolder?.title ?? '全部订阅'
  const hasVisibleUnread = useMemo(() => articles.some((article) => !article.is_read), [articles])
  const batchSummarySupported = contentView === 'all' && !query.trim() && !favoriteOnly
  const readerGridStyle = useMemo<CSSProperties>(
    () => ({
      gridTemplateColumns: `${readerLayout.sidebar}px ${RESIZER_WIDTH}px ${readerLayout.list}px ${RESIZER_WIDTH}px minmax(0, 1fr)`
    }),
    [readerLayout]
  )

  const refreshShell = useCallback(async () => {
    const [nextFeeds, nextFolders, nextMetrics] = await Promise.all([
      api<Feed[]>('/api/feeds'),
      api<Folder[]>('/api/folders'),
      api<Metrics>('/api/metrics')
    ])
    setFeeds(nextFeeds)
    setFolders(nextFolders)
    setMetrics(nextMetrics)
  }, [])

  const refreshKeywordRules = useCallback(async () => {
    setKeywordRules(await api<KeywordRule[]>('/api/keyword-rules'))
  }, [])

  const refreshMonitors = useCallback(async () => {
    setMonitors(await api<Monitor[]>('/api/monitors'))
  }, [])

  const refreshKeywordMatches = useCallback(async () => {
    const params = new URLSearchParams()
    params.set('limit', '100')
    if (keywordMatchRuleFilter) params.set('rule_id', keywordMatchRuleFilter)
    const nextMatches = await api<KeywordMatch[]>(`/api/keyword-matches?${params.toString()}`)
    setKeywordMatches(nextMatches)
    setSelectedKeywordMatchIds((current) =>
      current.filter((id) => nextMatches.some((match) => match.id === id))
    )
  }, [keywordMatchRuleFilter])

  const refreshArticles = useCallback(async (offset = 0, append = false) => {
    const requestGeneration = articleRequestGenerationRef.current + 1
    articleRequestGenerationRef.current = requestGeneration
    articleRequestControllerRef.current?.abort()
    const controller = new AbortController()
    articleRequestControllerRef.current = controller
    const params = new URLSearchParams()
    if (selectedFeedId) params.set('feed_id', selectedFeedId)
    if (!selectedFeedId && selectedFolderId) params.set('folder_id', selectedFolderId)
    if (unreadOnly) params.set('unread', 'true')
    if (favoriteOnly) params.set('favorite', 'true')
    params.set('limit', String(ARTICLE_PAGE_SIZE))
    params.set('offset', String(offset))
    params.set('sort', sortMode)
    if (query.trim()) params.set('q', query.trim())
    const path = query.trim()
      ? `/api/search?${params.toString()}`
      : contentView === 'today'
        ? `/api/brief/today?limit=${ARTICLE_PAGE_SIZE}&hours=72`
        : contentView === 'monitor' && selectedMonitorId
          ? `/api/monitors/${selectedMonitorId}/articles`
          : `/api/articles?${params.toString()}`
    let nextArticles: ArticleListItem[]
    try {
      nextArticles = await api<ArticleListItem[]>(path, { signal: controller.signal })
    } catch (err) {
      if (controller.signal.aborted) return
      throw err
    }
    if (requestGeneration !== articleRequestGenerationRef.current) return
    setArticles((current) => {
      if (!append) return nextArticles
      const known = new Set(current.map((article) => article.id))
      return [...current, ...nextArticles.filter((article) => !known.has(article.id))]
    })
    setHasMoreArticles(contentView === 'all' && nextArticles.length === ARTICLE_PAGE_SIZE)
    if (!append) {
      setSelectedArticleId((current) =>
        nextArticles.some((article) => article.id === current) ? current : (nextArticles[0]?.id ?? null)
      )
    }
  }, [contentView, favoriteOnly, query, selectedFeedId, selectedFolderId, selectedMonitorId, sortMode, unreadOnly])

  useEffect(() => () => {
    articleRequestControllerRef.current?.abort()
    for (const controller of summaryPollControllersRef.current.values()) controller.abort()
    summaryPollControllersRef.current.clear()
  }, [])

  useEffect(() => {
    if (activeView === 'now' || shellLoadedRef.current) return
    const request = shellRequestRef.current ?? Promise.all([refreshShell(), refreshMonitors()]).then(() => {
      shellLoadedRef.current = true
    })
    shellRequestRef.current = request
    void request
      .catch((err) => setError(err instanceof Error ? err.message : '资料加载失败'))
      .finally(() => {
        if (shellRequestRef.current === request) shellRequestRef.current = null
      })
  }, [activeView, refreshMonitors, refreshShell])

  useEffect(() => {
    function onPopState() {
      const nextView = readAppView()
      const nextArticleId = readArticleId()
      directArticleOpenRef.current = nextView === 'reader' && Boolean(nextArticleId)
      setQuery(readLibraryQuery())
      setSelectedArticleId(nextArticleId)
      setActiveView(nextView)
    }
    window.addEventListener('popstate', onPopState)
    return () => window.removeEventListener('popstate', onPopState)
  }, [])

  useEffect(() => {
    if (activeView !== 'reader') return
    if (directArticleOpenRef.current) {
      directArticleOpenRef.current = false
      return
    }
    refreshArticles(0, false).catch((err) => setError(err instanceof Error ? err.message : '文章加载失败'))
  }, [activeView, refreshArticles])

  useEffect(() => {
    if (activeView !== 'settings') return
    refreshKeywordRules().catch((err) => setError(err instanceof Error ? err.message : '关键词规则加载失败'))
  }, [activeView, refreshKeywordRules])

  useEffect(() => {
    if (activeView !== 'settings') return
    refreshKeywordMatches().catch((err) => setError(err instanceof Error ? err.message : '关键词命中加载失败'))
  }, [activeView, refreshKeywordMatches])

  useEffect(() => {
    if (activeView !== 'settings') return
    let active = true
    setSocialSourceCatalog(null)
    setSocialCatalogError(null)
    setSocialCatalogLoading(true)
    api<SocialSourceCatalog>('/api/social-sources')
      .then((catalog) => {
        if (active) setSocialSourceCatalog(catalog)
      })
      .catch((err) => {
        if (active) setSocialCatalogError(err instanceof Error ? err.message : '社媒来源目录加载失败')
      })
      .finally(() => {
        if (active) setSocialCatalogLoading(false)
      })
    return () => {
      active = false
    }
  }, [activeView])

  useEffect(() => {
    if (activeView !== 'reader') return
    if (!selectedArticleId) {
      setSelectedArticle(null)
      setShowTranslation(false)
      return
    }
    setSelectedArticle(null)
    setShowTranslation(false)
    const controller = new AbortController()
    api<ArticleDetail>(`/api/articles/${selectedArticleId}`, { signal: controller.signal })
      .then((article) => {
        setSelectedArticle(article)
        setFeeds((current) =>
          current.map((feed) =>
            feed.id === article.feed_id
              ? { ...feed, unread_count: Math.max(0, feed.unread_count - 1) }
              : feed
          )
        )
        setArticles((current) =>
          current.map((item) => (item.id === article.id ? { ...item, is_read: true } : item))
        )
      })
      .catch((err) => {
        if (err instanceof DOMException && err.name === 'AbortError') return
        setError(err instanceof Error ? err.message : '文章打开失败')
      })
    return () => controller.abort()
  }, [activeView, selectedArticleId])

  useEffect(() => {
    if (activeView !== 'reader') return
    if (!selectedArticle?.story_id || selectedArticle.story_source_count < 2) {
      setSelectedStory(null)
      return
    }
    const controller = new AbortController()
    api<StoryCluster>(`/api/stories/${selectedArticle.story_id}`, { signal: controller.signal })
      .then(setSelectedStory)
      .catch((err) => {
        if (err instanceof DOMException && err.name === 'AbortError') return
        setSelectedStory(null)
      })
    return () => controller.abort()
  }, [activeView, selectedArticle?.story_id, selectedArticle?.story_source_count])

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (activeView !== 'reader') return
      if (event.defaultPrevented || event.repeat || event.ctrlKey || event.metaKey || event.altKey) return
      if (
        event.target instanceof Element &&
        event.target.closest('input, textarea, select, button, a, [contenteditable="true"]')
      ) return
      const index = articles.findIndex((article) => article.id === selectedArticleId)
      const key = event.key.toLowerCase()
      if (key === 'j' && articles[index + 1]) {
        event.preventDefault()
        setSelectedArticleId(articles[index + 1].id)
      }
      if (key === 'k' && articles[index - 1]) {
        event.preventDefault()
        setSelectedArticleId(articles[index - 1].id)
      }
      if (
        key === 's'
        && selectedArticle?.id === selectedArticleId
        && !summaryPollControllersRef.current.has(selectedArticle.id)
      ) {
        event.preventDefault()
        void summarizeSelected()
      }
      if (key === 'f' && selectedArticle) {
        event.preventDefault()
        void toggleFavorite()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  useEffect(() => {
    try {
      window.localStorage.setItem(READER_LAYOUT_STORAGE_KEY, JSON.stringify(readerLayout))
    } catch {
      // Ignore private browsing or locked-down storage.
    }
  }, [readerLayout])

  useEffect(() => {
    function onResize() {
      if (window.matchMedia('(max-width: 820px)').matches) return
      setReaderLayout((layout) => clampReaderLayout(layout, window.innerWidth))
    }
    window.addEventListener('resize', onResize)
    return () => window.removeEventListener('resize', onResize)
  }, [])

  const groupedFeeds = useMemo<FeedGroup[]>(() => {
    return folders.map((folder) => ({
      folder,
      feeds: feeds.filter((feed) => feed.folder_id === folder.id),
      unreadCount: feeds
        .filter((feed) => feed.folder_id === folder.id)
        .reduce((total, feed) => total + feed.unread_count, 0)
    }))
  }, [feeds, folders])
  const ungroupedFeeds = useMemo(() => feeds.filter((feed) => feed.folder_id === null), [feeds])
  const presetGroups = useMemo<PresetGroup[]>(() => {
    const categories = Array.from(new Set(READY_MADE_FEEDS.map((feed) => feed.category))).sort((left, right) => {
      const leftIndex = PRESET_CATEGORY_ORDER.indexOf(left)
      const rightIndex = PRESET_CATEGORY_ORDER.indexOf(right)
      if (leftIndex === -1 && rightIndex === -1) return left.localeCompare(right, 'zh-CN')
      if (leftIndex === -1) return 1
      if (rightIndex === -1) return -1
      return leftIndex - rightIndex
    })
    return categories.map((category) => ({
      category,
      feeds: READY_MADE_FEEDS.filter((feed) => feed.category === category)
    }))
  }, [])

  async function guarded(action: () => Promise<void>, success?: string) {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      await action()
      if (success) setNotice(success)
    } catch (err) {
      setError(err instanceof Error ? err.message : '操作失败')
    } finally {
      setBusy(false)
    }
  }

  async function addFeed(event: FormEvent) {
    event.preventDefault()
    await guarded(async () => {
      const feed = await api<Feed>('/api/feeds', {
        method: 'POST',
        body: JSON.stringify({ feed_url: feedUrl, folder_id: newFeedFolderId || null })
      })
      setFeedUrl('')
      selectFeed(feed.id)
      await refreshShell()
    }, '订阅已加入，后台正在抓取')
  }

  async function addFolder(event: FormEvent) {
    event.preventDefault()
    await guarded(async () => {
      const folder = await api<Folder>('/api/folders', {
        method: 'POST',
        body: JSON.stringify({ title: folderName })
      })
      setFolderName('')
      setNewFeedFolderId(folder.id)
      await refreshShell()
    }, '文件夹已创建')
  }

  async function ensureFolder(title: string) {
    const existing = folders.find((folder) => folder.title.toLowerCase() === title.toLowerCase())
    if (existing) return existing.id

    const folder = await api<Folder>('/api/folders', {
      method: 'POST',
      body: JSON.stringify({ title })
    })
    setFolders((current) => (current.some((item) => item.id === folder.id) ? current : [...current, folder]))
    return folder.id
  }

  async function addSocialSource(adapter: SocialSourceAdapter, path: string, title?: string) {
    await guarded(async () => {
      const folderId = await ensureFolder('社交媒体')
      const feed = await api<Feed>('/api/social-sources', {
        method: 'POST',
        body: JSON.stringify({ adapter, path: path.trim(), title, folder_id: folderId })
      })
      if (adapter === 'activitypub') setActivityPubHandle('')
      if (adapter === 'rsshub') setRssHubRoute('')
      selectFeed(feed.id)
      await refreshShell()
      setNotice(`测试成功，已将 ${feed.title} 添加到「社交媒体」`)
    })
  }

  async function addActivityPubSource(event: FormEvent) {
    event.preventDefault()
    await addSocialSource('activitypub', activityPubHandle)
  }

  async function addRssHubSource(event: FormEvent) {
    event.preventDefault()
    await addSocialSource('rsshub', rssHubRoute)
  }

  async function createPresetFeed(preset: ReadyMadeFeed, folderId: string): Promise<Feed> {
    const metadata = presetSourceMetadata(preset)
    if (preset.rsshubPath) {
      if (!socialSourceCatalog?.rsshub_enabled) throw new Error('RSSHub 尚未启用')
      return api<Feed>('/api/social-sources', {
        method: 'POST',
        body: JSON.stringify({
          adapter: 'rsshub',
          path: preset.rsshubPath,
          title: preset.title,
          folder_id: folderId,
          ...metadata
        })
      })
    }
    if (!preset.url) throw new Error('推荐订阅缺少 URL')
    return api<Feed>('/api/feeds', {
      method: 'POST',
      body: JSON.stringify({
        feed_url: preset.url,
        title: preset.title,
        folder_id: folderId,
        ...metadata
      })
    })
  }

  async function addReadyFeed(preset: ReadyMadeFeed) {
    await guarded(async () => {
      if (preset.rsshubPath && !socialSourceCatalog?.rsshub_enabled) throw new Error('RSSHub 尚未启用')
      const folderId = await ensureFolder(preset.category)
      const existing = feeds.find((feed) => matchesPresetFeed(feed, preset))
      if (existing && existing.folder_id === folderId) {
        selectFeed(existing.id)
        setNotice(`${preset.title} 已在订阅列表中`)
        return
      }

      const feed = await createPresetFeed(preset, folderId)
      selectFeed(feed.id)
      await refreshShell()
      setNotice(existing ? `已将 ${preset.title} 归入 ${preset.category}` : `已添加 ${preset.title}，后台正在抓取`)
    })
  }

  async function addPresetGroup(category: string) {
    await guarded(async () => {
      const folderId = await ensureFolder(category)
      let added = 0
      let updated = 0
      let unavailable = 0
      const failed: string[] = []
      for (const preset of READY_MADE_FEEDS.filter((feed) => feed.category === category)) {
        if (preset.rsshubPath && !socialSourceCatalog?.rsshub_enabled) {
          unavailable += 1
          continue
        }
        const existing = feeds.find((feed) => matchesPresetFeed(feed, preset))
        if (existing && existing.folder_id === folderId) continue
        try {
          await createPresetFeed(preset, folderId)
        } catch {
          failed.push(preset.title)
          continue
        }
        if (existing) {
          updated += 1
        } else {
          added += 1
        }
      }
      selectFolder(folderId)
      await refreshShell()
      setNotice(
        added || updated || unavailable || failed.length
          ? `已添加 ${added} 个、归类 ${updated} 个${category}订阅${unavailable ? `；${unavailable} 个 RSSHub 来源未启用` : ''}${failed.length ? `；未添加：${failed.join('、')}` : ''}`
          : `${category}订阅都已存在`
      )
    })
  }

  async function fetchFeed(feedId: string, title?: string) {
    await guarded(async () => {
      await api(`/api/feeds/${feedId}/fetch`, { method: 'POST' })
      await refreshShell()
    }, title ? `已加入 ${title} 抓取队列` : '已加入抓取队列')
  }

  async function fetchSelectedFeed() {
    if (!selectedFeedId) return
    await fetchFeed(selectedFeedId)
  }

  async function deleteFeed(feed: Feed) {
    if (!window.confirm(`删除订阅「${feed.title}」及其已抓取文章？`)) return
    await guarded(async () => {
      await api(`/api/feeds/${feed.id}`, { method: 'DELETE' })
      const deletedSelectedFeed = selectedFeedId === feed.id
      if (deletedSelectedFeed) {
        selectAllFeeds()
      }
      await refreshShell()
      if (!deletedSelectedFeed) {
        await refreshArticles()
      }
    }, `已删除 ${feed.title}`)
  }

  async function summarizeSelected() {
    if (!selectedArticle || selectedArticle.id !== selectedArticleId) return
    const articleId = selectedArticle.id
    if (summaryPollControllersRef.current.has(articleId)) {
      setNotice('这篇文章的摘要任务正在处理中')
      return
    }
    const pollController = new AbortController()
    summaryPollControllersRef.current.set(articleId, pollController)
    setSummarizingIds((current) => new Set(current).add(articleId))
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const result = await api<{ queued: number }>('/api/summaries', {
        method: 'POST',
        body: JSON.stringify({ article_id: articleId })
      })
      setNotice(result.queued > 0 ? '已加入摘要队列' : '摘要任务已在处理中')
      void pollSummary(articleId, pollController).finally(() => {
        if (summaryPollControllersRef.current.get(articleId) === pollController) {
          summaryPollControllersRef.current.delete(articleId)
          setSummarizingIds((current) => {
            const next = new Set(current)
            next.delete(articleId)
            return next
          })
        }
      })
    } catch (err) {
      summaryPollControllersRef.current.delete(articleId)
      setSummarizingIds((current) => {
        const next = new Set(current)
        next.delete(articleId)
        return next
      })
      setError(err instanceof Error ? err.message : '摘要入队失败')
    } finally {
      setBusy(false)
    }
  }

  async function translateSelected() {
    if (!selectedArticle || selectedArticle.id !== selectedArticleId) return
    const articleId = selectedArticle.id
    if (translatingIds.has(articleId)) {
      setNotice('这篇文章正在翻译')
      return
    }
    setTranslatingIds((current) => new Set(current).add(articleId))
    setError(null)
    setNotice('正在翻译全文…')
    try {
      const translation = await api<ArticleTranslation>('/api/translations', {
        method: 'POST',
        body: JSON.stringify({ article_id: articleId, target_language: 'zh-Hans' })
      })
      setSelectedArticle((current) => (
        current?.id === articleId ? { ...current, translation } : current
      ))
      setShowTranslation(true)
      setNotice('翻译完成，可随时切换回原文')
    } catch (err) {
      setError(err instanceof Error ? err.message : '翻译失败')
      setNotice(null)
    } finally {
      setTranslatingIds((current) => {
        const next = new Set(current)
        next.delete(articleId)
        return next
      })
    }
  }

  async function pollSummary(articleId: string, pollController: AbortController) {
    const deadline = Date.now() + SUMMARY_POLL_DEADLINE_MS
    while (!pollController.signal.aborted && Date.now() < deadline) {
      const waitTime = Math.min(SUMMARY_POLL_INTERVAL_MS, deadline - Date.now())
      if (waitTime <= 0 || !await waitWithAbort(waitTime, pollController.signal)) return
      const requestTime = Math.min(SUMMARY_REQUEST_TIMEOUT_MS, deadline - Date.now())
      if (requestTime <= 0) break
      const requestController = new AbortController()
      const abortRequest = () => requestController.abort()
      pollController.signal.addEventListener('abort', abortRequest, { once: true })
      const timeout = window.setTimeout(abortRequest, requestTime)
      try {
        const summary = await api<Summary | null>(`/api/summaries/${articleId}`, {
          signal: requestController.signal
        })
        setSelectedArticle((current) => (
          current?.id === articleId ? { ...current, summary } : current
        ))
        if (summary?.status === 'done' || summary?.status === 'failed') {
          setArticles((current) => current.map((item) => (
            item.id === articleId ? { ...item, has_summary: summary.status === 'done' } : item
          )))
          return
        }
      } catch (err) {
        if (pollController.signal.aborted) return
        if (
          err instanceof ApiError
          && err.status >= 400
          && err.status < 500
          && err.status !== 408
          && err.status !== 429
        ) {
          if (selectedArticleIdRef.current === articleId) setError(err.message)
          return
        }
        // Request timeout, network errors, 429 and 5xx are transient; keep polling.
      } finally {
        window.clearTimeout(timeout)
        pollController.signal.removeEventListener('abort', abortRequest)
      }
    }
    if (!pollController.signal.aborted && selectedArticleIdRef.current === articleId) {
      setNotice('摘要仍在后台处理，可稍后重新打开文章查看')
    }
  }

  async function saveMonitor(monitorId: string | null, payload: MonitorRequest) {
    await guarded(async () => {
      const monitor = await api<Monitor>(monitorId ? `/api/monitors/${monitorId}` : '/api/monitors', {
        method: monitorId ? 'PATCH' : 'POST',
        body: JSON.stringify(payload)
      })
      await refreshMonitors()
      selectMonitor(monitor.id)
    }, monitorId ? '主题监控已更新' : '主题监控已创建')
  }

  async function deleteMonitor(monitor: Monitor) {
    if (!window.confirm(`删除主题监控「${monitor.name}」？`)) return
    await guarded(async () => {
      await api(`/api/monitors/${monitor.id}`, { method: 'DELETE' })
      if (selectedMonitorId === monitor.id) selectToday()
      await refreshMonitors()
    }, `已删除 ${monitor.name}`)
  }

  async function updateSourcePreference(feed: Feed, patch: FeedPreferencePatch) {
    await guarded(async () => {
      await api<Feed>(`/api/feeds/${feed.id}`, {
        method: 'PATCH',
        body: JSON.stringify(patch)
      })
      await refreshShell()
      await refreshArticles(0, false)
    }, patch.muted === true ? `已静音 ${feed.title}` : patch.muted === false ? `已恢复 ${feed.title}` : patch.editorial_profile ? `已更新 ${feed.title} 的内容类型` : `已调整 ${feed.title} 的优先级`)
  }

  async function summarizeUnread() {
    await guarded(async () => {
      const result = await api<{ queued: number }>('/api/summaries/batch', {
        method: 'POST',
        body: JSON.stringify({
          feed_id: selectedFeedId,
          folder_id: selectedFeedId ? null : selectedFolderId,
          unread_only: true,
          limit: 25
        })
      })
      setNotice(`已加入 ${result.queued} 篇未读摘要任务`)
    })
  }

  async function saveKeywordRule(ruleId: string | null, payload: KeywordRuleRequest) {
    await guarded(async () => {
      await api<KeywordRule>(ruleId ? `/api/keyword-rules/${ruleId}` : '/api/keyword-rules', {
        method: ruleId ? 'PATCH' : 'POST',
        body: JSON.stringify(payload)
      })
      await refreshKeywordRules()
      await refreshKeywordMatches()
    }, ruleId ? '关键词规则已更新' : '关键词规则已创建')
  }

  async function scanKeywordRule(rule: KeywordRule) {
    await guarded(async () => {
      const result = await api<KeywordScanResponse>(`/api/keyword-rules/${rule.id}/scan`, {
        method: 'POST',
        body: JSON.stringify({ limit: 500 })
      })
      await refreshKeywordRules()
      await refreshKeywordMatches()
      setNotice(`已扫描 ${result.scanned} 篇，命中 ${result.matched} 篇，新增 ${result.new_matches} 条`)
    })
  }

  async function deleteKeywordRule(rule: KeywordRule) {
    if (!window.confirm(`删除关键词规则「${rule.name}」及其命中记录？`)) return
    await guarded(async () => {
      await api(`/api/keyword-rules/${rule.id}`, { method: 'DELETE' })
      if (keywordMatchRuleFilter === rule.id) {
        setKeywordMatchRuleFilter('')
      }
      await refreshKeywordRules()
      await refreshKeywordMatches()
    }, `已删除 ${rule.name}`)
  }

  function toggleKeywordMatchSelection(matchId: string) {
    setSelectedKeywordMatchIds((current) =>
      current.includes(matchId) ? current.filter((id) => id !== matchId) : [...current, matchId]
    )
  }

  function selectVisibleKeywordMatches() {
    setSelectedKeywordMatchIds(keywordMatches.map((match) => match.id))
  }

  async function runKeywordMatchAction(summarize: boolean, telegram: boolean) {
    if (selectedKeywordMatchIds.length === 0) {
      setNotice(null)
      setError('请选择至少一个关键词命中')
      return
    }
    await guarded(async () => {
      const result = await api<KeywordActionResponse>('/api/keyword-matches/actions', {
        method: 'POST',
        body: JSON.stringify({
          match_ids: selectedKeywordMatchIds,
          summarize,
          telegram
        })
      })
      await refreshKeywordRules()
      await refreshKeywordMatches()
      setNotice(
        `摘要 ${result.queued_summaries} 条，TG 成功 ${result.telegram_sent} 条，失败 ${result.telegram_failed} 条`
      )
    })
  }

  async function toggleFavorite() {
    if (!selectedArticle) return
    const next = !selectedArticle.is_favorite
    await guarded(async () => {
      await api(`/api/articles/${selectedArticle.id}`, {
        method: 'PATCH',
        body: JSON.stringify({ is_favorite: next })
      })
      setSelectedArticle({ ...selectedArticle, is_favorite: next })
      setArticles((current) =>
        current.map((article) =>
          article.id === selectedArticle.id ? { ...article, is_favorite: next } : article
        )
      )
    })
  }

  async function importOpml() {
    await guarded(async () => {
      const result = await api<{ queued: number }>('/api/opml/import', {
        method: 'POST',
        headers: { 'Content-Type': 'text/xml' },
        body: opmlText
      })
      setOpmlText('')
      await refreshShell()
      setNotice(`已导入 ${result.queued} 个订阅源`)
    })
  }

  async function loadOpmlFile(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0]
    if (!file) return
    try {
      setOpmlText(await file.text())
      setError(null)
      setNotice(`已读取 ${file.name}`)
    } catch (err) {
      setNotice(null)
      setError(err instanceof Error ? err.message : 'OPML 文件读取失败')
    } finally {
      event.target.value = ''
    }
  }

  async function logout() {
    await api('/api/auth/logout', { method: 'POST' })
    onLogout()
  }

  function startPaneResize(target: ResizeTarget, event: ReactPointerEvent<HTMLDivElement>) {
    if (window.matchMedia('(max-width: 820px)').matches) return
    event.preventDefault()
    const shellWidth = event.currentTarget.parentElement?.getBoundingClientRect().width ?? window.innerWidth
    const startX = event.clientX
    const startLayout = readerLayout
    document.body.classList.add('is-resizing-panes')

    function onPointerMove(moveEvent: PointerEvent) {
      setReaderLayout(resizeReaderLayout(startLayout, target, moveEvent.clientX - startX, shellWidth))
    }

    function onPointerUp() {
      document.body.classList.remove('is-resizing-panes')
      window.removeEventListener('pointermove', onPointerMove)
      window.removeEventListener('pointerup', onPointerUp)
    }

    window.addEventListener('pointermove', onPointerMove)
    window.addEventListener('pointerup', onPointerUp)
  }

  function resizeWithKeyboard(target: ResizeTarget, event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return
    event.preventDefault()
    const step = event.shiftKey ? 48 : 16
    const direction = event.key === 'ArrowRight' ? 1 : -1
    setReaderLayout((layout) => resizeReaderLayout(layout, target, step * direction, window.innerWidth))
  }

  function resetReaderLayout() {
    setReaderLayout(clampReaderLayout(DEFAULT_READER_LAYOUT, window.innerWidth))
  }

  function selectAllFeeds() {
    setContentView('all')
    setSelectedFeedId(null)
    setSelectedFolderId(null)
    setSelectedMonitorId(null)
    setSelectedArticleId(null)
    setMobilePane('articles')
  }

  function selectFolder(folderId: string) {
    setContentView('all')
    setSelectedFeedId(null)
    setSelectedFolderId(folderId)
    setSelectedMonitorId(null)
    setSelectedArticleId(null)
    setMobilePane('articles')
  }

  function selectFeed(feedId: string) {
    setContentView('all')
    setSelectedFolderId(null)
    setSelectedFeedId(feedId)
    setSelectedMonitorId(null)
    setSelectedArticleId(null)
    setMobilePane('articles')
  }

  function selectToday() {
    setContentView('today')
    setSelectedFeedId(null)
    setSelectedFolderId(null)
    setSelectedMonitorId(null)
    setSelectedArticleId(null)
    setQuery('')
    setMobilePane('articles')
  }

  function selectMonitor(monitorId: string) {
    setContentView('monitor')
    setSelectedFeedId(null)
    setSelectedFolderId(null)
    setSelectedMonitorId(monitorId)
    setSelectedArticleId(null)
    setQuery('')
    setMobilePane('articles')
  }

  async function loadMoreArticles() {
    if (!hasMoreArticles || busy) return
    await guarded(async () => {
      await refreshArticles(articles.length, true)
    })
  }

  function scrollReaderToTop(behavior: ScrollBehavior = 'smooth') {
    if (window.matchMedia('(max-width: 820px)').matches) {
      window.scrollTo({ top: 0, behavior: resolveScrollBehavior(behavior) })
      return
    }
    document.querySelector('.reader-pane')?.scrollTo({ top: 0, behavior: resolveScrollBehavior(behavior) })
  }

  function scrollArticleListToTop(behavior: ScrollBehavior = 'smooth') {
    document.querySelector('.article-list')?.scrollTo({ top: 0, behavior: resolveScrollBehavior(behavior) })
  }

  function scrollArticleRowIntoView(articleId: string) {
    document.getElementById(`article-row-${articleId}`)?.scrollIntoView({
      block: 'center',
      behavior: resolveScrollBehavior('smooth'),
    })
  }

  function jumpToFirstUnread() {
    const unreadArticle = articles.find((article) => !article.is_read)
    if (!unreadArticle) {
      setError(null)
      setNotice('当前列表没有未读文章')
      return
    }

    setError(null)
    setNotice(null)
    setSelectedArticleId(unreadArticle.id)
    requestAnimationFrame(() => {
      scrollArticleRowIntoView(unreadArticle.id)
      scrollReaderToTop()
    })
  }

  function jumpToTop() {
    scrollArticleListToTop('auto')
    scrollReaderToTop('auto')
  }

  if (activeView === 'now') {
    return (
      <WorldRadar
        userName={user.username}
        theme={theme}
        onToggleTheme={onToggleTheme}
        onOpenSettings={() => navigateToView('settings')}
        onOpenLibrary={(nextQuery, articleId) => {
          setQuery(nextQuery ?? '')
          if (articleId) {
            directArticleOpenRef.current = true
            setSelectedArticleId(articleId)
            setMobilePane('reader')
          } else {
            setMobilePane('articles')
          }
          navigateToView('reader', { query: nextQuery ?? '', articleId: articleId ?? null })
        }}
      />
    )
  }

  if (activeView === 'settings') {
    return (
      <SettingsPage
        theme={theme}
        feeds={feeds}
        folders={folders}
        groupedFeeds={groupedFeeds}
        ungroupedFeeds={ungroupedFeeds}
        presetGroups={presetGroups}
        keywordRules={keywordRules}
        keywordMatches={keywordMatches}
        monitors={monitors}
        keywordMatchRuleFilter={keywordMatchRuleFilter}
        selectedKeywordMatchIds={selectedKeywordMatchIds}
        feedUrl={feedUrl}
        newFeedFolderId={newFeedFolderId}
        folderName={folderName}
        opmlText={opmlText}
        socialSourceCatalog={socialSourceCatalog}
        socialCatalogLoading={socialCatalogLoading}
        socialCatalogError={socialCatalogError}
        activityPubHandle={activityPubHandle}
        rssHubRoute={rssHubRoute}
        busy={busy}
        notice={notice}
        error={error}
        opmlFileInputRef={opmlFileInputRef}
        onBack={() => navigateToView('now', { mode: 'replace' })}
        onToggleTheme={onToggleTheme}
        onFeedUrlChange={setFeedUrl}
        onNewFeedFolderChange={setNewFeedFolderId}
        onFolderNameChange={setFolderName}
        onOpmlTextChange={setOpmlText}
        onActivityPubHandleChange={setActivityPubHandle}
        onRssHubRouteChange={setRssHubRoute}
        onAddFeed={addFeed}
        onAddFolder={addFolder}
        onAddActivityPubSource={addActivityPubSource}
        onAddRssHubSource={addRssHubSource}
        onAddRssHubPresetSource={(source) => addSocialSource('rsshub', source.path, source.title)}
        onAddOpenCliSource={(source) => addSocialSource('opencli', source.name, source.title)}
        onAddReadyFeed={addReadyFeed}
        onAddPresetGroup={addPresetGroup}
        onFetchFeed={fetchFeed}
        onDeleteFeed={deleteFeed}
        onUpdateSourcePreference={updateSourcePreference}
        onSaveMonitor={saveMonitor}
        onDeleteMonitor={deleteMonitor}
        onSaveKeywordRule={saveKeywordRule}
        onScanKeywordRule={scanKeywordRule}
        onDeleteKeywordRule={deleteKeywordRule}
        onKeywordMatchRuleFilterChange={setKeywordMatchRuleFilter}
        onToggleKeywordMatchSelection={toggleKeywordMatchSelection}
        onSelectVisibleKeywordMatches={selectVisibleKeywordMatches}
        onRunKeywordMatchAction={runKeywordMatchAction}
        onImportOpml={importOpml}
        onLoadOpmlFile={loadOpmlFile}
        onLogout={logout}
      />
    )
  }

  return (
    <>
      <a className="skip-link" href="#main-content">跳到正文</a>
      <nav className="mobile-reader-nav" aria-label="移动端阅读导航">
        <button className={mobilePane === 'sources' ? 'active' : ''} onClick={() => setMobilePane('sources')}>来源</button>
        <button className={mobilePane === 'articles' ? 'active' : ''} onClick={() => setMobilePane('articles')}>文章</button>
        <button className={mobilePane === 'reader' ? 'active' : ''} onClick={() => setMobilePane('reader')} disabled={!selectedArticleId}>正文</button>
      </nav>
      <div className={`reader-shell mobile-${mobilePane}`} style={readerGridStyle}>
      <aside id="subscription-pane" className="sidebar">
        <div className="app-mark">
          <div className="app-mark-brand">
            <Radar size={24} />
            <div>
              <strong>全部资料</strong>
              <button type="button" onClick={() => navigateToView('now', { mode: 'replace' })}>返回信息雷达</button>
            </div>
          </div>
          <div className="app-mark-actions">
            <ThemeToggle theme={theme} onToggleTheme={onToggleTheme} />
            <button
              type="button"
              className="theme-toggle"
              onClick={() => navigateToView('settings')}
              aria-label="打开设置"
              title="设置"
            >
              <Settings size={16} />
            </button>
          </div>
        </div>

        <button
          className={contentView === 'today' ? 'feed-row active' : 'feed-row'}
          onClick={selectToday}
          aria-pressed={contentView === 'today'}
        >
          <span>今日重点</span>
        </button>

        {monitors.length > 0 && (
          <div className="feed-section monitor-section">
            <span className="sidebar-section-label">主题监控</span>
            {monitors.filter((monitor) => monitor.enabled).map((monitor) => (
              <button
                key={monitor.id}
                className={contentView === 'monitor' && selectedMonitorId === monitor.id ? 'feed-row active' : 'feed-row'}
                onClick={() => selectMonitor(monitor.id)}
                aria-pressed={contentView === 'monitor' && selectedMonitorId === monitor.id}
                title={monitor.query}
              >
                <span>{monitor.name}</span>
                <b>⌁</b>
              </button>
            ))}
          </div>
        )}

        <button
          className={contentView === 'all' && selectedFeedId === null && selectedFolderId === null ? 'feed-row active' : 'feed-row'}
          onClick={selectAllFeeds}
          aria-pressed={contentView === 'all' && selectedFeedId === null && selectedFolderId === null}
        >
          <span>全部订阅</span>
          {metrics && <b title="未读">{metrics.unread}</b>}
        </button>

        {ungroupedFeeds.length > 0 && (
          <div className="feed-section">
            {ungroupedFeeds.map((feed) => (
              <FeedRow key={feed.id} feed={feed} active={feed.id === selectedFeedId} onClick={selectFeed} />
            ))}
          </div>
        )}

        {groupedFeeds.map((group) => (
          <div className="feed-section" key={group.folder.id}>
            <FolderRow
              folder={group.folder}
              active={group.folder.id === selectedFolderId && selectedFeedId === null}
              unreadCount={group.unreadCount}
              onClick={selectFolder}
            />
            {group.feeds.map((feed) => (
              <FeedRow key={feed.id} feed={feed} active={feed.id === selectedFeedId} onClick={selectFeed} nested />
            ))}
          </div>
        ))}

        {feeds.length === 0 && <div className="sidebar-empty">打开设置页添加订阅后，自选分类会显示在这里。</div>}
      </aside>

      <div
        className="pane-resizer"
        role="separator"
        aria-label="调整订阅栏宽度"
        aria-orientation="vertical"
        aria-controls="subscription-pane"
        aria-valuemin={MIN_SIDEBAR_WIDTH}
        aria-valuemax={MAX_SIDEBAR_WIDTH}
        aria-valuenow={Math.round(readerLayout.sidebar)}
        tabIndex={0}
        title="拖动调整订阅栏宽度，双击恢复默认"
        onPointerDown={(event) => startPaneResize('sidebar', event)}
        onKeyDown={(event) => resizeWithKeyboard('sidebar', event)}
        onDoubleClick={resetReaderLayout}
      />

      <section id="article-list-pane" className="article-list-pane">
        <header className="toolbar">
          <div className="search-box">
            <Search size={16} />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="搜索标题、正文、订阅源"
              aria-label="搜索文章"
            />
          </div>
          <div className="toolbar-actions">
            <button className="icon-button" onClick={jumpToFirstUnread} disabled={!hasVisibleUnread} title="跳到第一篇未读" aria-label="跳到第一篇未读">
              <CircleDot size={16} />
            </button>
            <button className="icon-button" onClick={jumpToTop} title="回到顶部" aria-label="回到顶部">
              <ArrowUp size={16} />
            </button>
            <button className={unreadOnly ? 'icon-button active' : 'icon-button'} onClick={() => setUnreadOnly(!unreadOnly)} title="筛选未读" aria-label="筛选未读" aria-pressed={unreadOnly}>
              <Circle size={16} />
            </button>
            <button className={favoriteOnly ? 'icon-button active' : 'icon-button'} onClick={() => setFavoriteOnly(!favoriteOnly)} title="收藏" aria-label="筛选收藏" aria-pressed={favoriteOnly}>
              <Star size={16} />
            </button>
            <button className="icon-button" onClick={fetchSelectedFeed} disabled={!selectedFeedId || busy} title="抓取当前订阅" aria-label="抓取当前订阅">
              <RefreshCw size={16} />
            </button>
          </div>
        </header>

        <div className="context-line">
          <div className="context-title">
            <strong>{contextTitle}</strong>
            <small>{articles.length} 篇 · {sortMode === 'priority' ? '质量优先' : '时间优先'}</small>
          </div>
          <div className="context-actions">
            {contentView === 'all' && (
              <button onClick={() => setSortMode((current) => current === 'priority' ? 'latest' : 'priority')} title="切换排序">
                {sortMode === 'priority' ? '质量优先' : '时间优先'}
              </button>
            )}
            {batchSummarySupported && (
              <button onClick={summarizeUnread} disabled={busy} title="批量总结当前订阅范围内的未读文章">
                <Sparkles size={15} />
                总结未读
              </button>
            )}
          </div>
        </div>

        {(notice || error) && (
          <div
            className={error ? 'status-line error' : 'status-line'}
            role={error ? 'alert' : 'status'}
            aria-live={error ? 'assertive' : 'polite'}
          >
            {error ?? notice}
          </div>
        )}

        <div className="article-list">
          {articles.length === 0 ? (
            <div className="empty-state">暂无文章。添加订阅或调整筛选条件。</div>
          ) : (
            articles.map((article) => (
              <button
                id={`article-row-${article.id}`}
                key={article.id}
                className={`article-row ${article.is_read ? 'read' : 'unread'}${
                  article.id === selectedArticleId ? ' active' : ''
                }`}
                onClick={() => {
                  setSelectedArticleId(article.id)
                  setMobilePane('reader')
                }}
                aria-current={article.id === selectedArticleId ? 'true' : undefined}
              >
                <div className="article-row-top">
                  {article.is_read ? <CheckCircle2 size={14} /> : <Circle size={14} />}
                  <span title={article.feed_title}>{shortSourceName(article.feed_title)}</span>
                  <time>{formatDate(article.published_at ?? article.fetched_at)}</time>
                </div>
                <strong>{article.title}</strong>
                <p>{article.excerpt}</p>
                <div className="article-flags">
                  <span className="quality-flag">质量 {article.quality_score}</span>
                  {article.story_source_count > 1 && <span>{article.story_source_count} 个来源</span>}
                  {article.priority_reasons.slice(0, 2).map((reason) => <span key={reason}>{priorityReasonLabel(reason)}</span>)}
                  {article.is_favorite && <span>收藏</span>}
                  {article.has_summary && <span>摘要</span>}
                </div>
              </button>
            ))
          )}
          {hasMoreArticles && (
            <button className="load-more-button" type="button" onClick={loadMoreArticles} disabled={busy}>
              {busy ? '加载中…' : '加载更多'}
            </button>
          )}
        </div>
      </section>

      <div
        className="pane-resizer"
        role="separator"
        aria-label="调整文章列表宽度"
        aria-orientation="vertical"
        aria-controls="article-list-pane"
        aria-valuemin={MIN_ARTICLE_LIST_WIDTH}
        aria-valuemax={MAX_ARTICLE_LIST_WIDTH}
        aria-valuenow={Math.round(readerLayout.list)}
        tabIndex={0}
        title="拖动调整文章列表宽度，双击恢复默认"
        onPointerDown={(event) => startPaneResize('list', event)}
        onKeyDown={(event) => resizeWithKeyboard('list', event)}
        onDoubleClick={resetReaderLayout}
      />

      <main id="main-content" className="reader-pane" tabIndex={-1}>
        {selectedArticle ? (
          <article>
            <button type="button" className="mobile-back-button" onClick={() => setMobilePane('articles')}>
              <ArrowLeft size={16} /> 返回文章
            </button>
            <header className="reader-header">
              <div>
                <span className="feed-chip" title={selectedArticle.feed_title}>{shortSourceName(selectedArticle.feed_title)}</span>
                <h1>
                  {showTranslation && selectedArticle.translation?.status === 'done'
                    ? selectedArticle.translation.translated_title || selectedArticle.title
                    : selectedArticle.title}
                </h1>
                <p>
                  {selectedArticle.author ? `${selectedArticle.author} · ` : ''}
                  {formatDate(selectedArticle.published_at ?? selectedArticle.fetched_at)}
                </p>
              </div>
              <div className="reader-actions">
                <button
                  onClick={toggleFavorite}
                  title="收藏"
                  aria-label="收藏文章"
                  aria-pressed={selectedArticle.is_favorite}
                >
                  <Star size={17} fill={selectedArticle.is_favorite ? 'currentColor' : 'none'} />
                </button>
                <button
                  onClick={summarizeSelected}
                  disabled={busy || summarizingIds.has(selectedArticle.id)}
                  title={summarizingIds.has(selectedArticle.id) ? '摘要处理中' : '生成摘要'}
                  aria-label="生成文章摘要"
                >
                  <Sparkles size={17} />
                </button>
                <button
                  onClick={() => {
                    if (selectedArticle.translation?.status === 'done') {
                      setShowTranslation((current) => !current)
                    } else {
                      void translateSelected()
                    }
                  }}
                  disabled={translatingIds.has(selectedArticle.id)}
                  title={translatingIds.has(selectedArticle.id)
                    ? '正在翻译'
                    : selectedArticle.translation?.status === 'done'
                      ? showTranslation ? '显示原文' : '显示中文翻译'
                      : selectedArticle.translation?.status === 'failed'
                        ? '重试全文翻译'
                        : '翻译全文为中文'}
                  aria-label={showTranslation ? '显示文章原文' : '翻译全文或显示中文译文'}
                  aria-pressed={showTranslation}
                >
                  <Languages size={17} />
                </button>
                <a href={selectedArticle.url} target="_blank" rel="noreferrer">
                  原文
                </a>
                {selectedArticle.extraction_error && !selectedArticle.extraction_error.includes('Feed 未提供正文') && (
                  <a
                    href={`https://www.removepaywall.com/search?url=${encodeURIComponent(selectedArticle.url)}`}
                    target="_blank"
                    rel="noreferrer"
                    title="原文抓取失败，通过公开存档查找全文"
                  >
                    免费阅读
                  </a>
                )}
              </div>
            </header>

            <SummaryPanel article={selectedArticle} />

            {selectedStory && selectedStory.articles.length > 1 && (
              <section className="story-sources" aria-label="同一事件的其他来源">
                <div className="story-sources-title">
                  <strong>同一事件，多源核对</strong>
                  <span>{selectedStory.source_count} 个来源 · 聚合质量 {selectedStory.quality_score}</span>
                </div>
                {selectedStory.articles.map((article) => (
                  <button
                    key={article.id}
                    type="button"
                    className={article.id === selectedArticle.id ? 'active' : ''}
                    onClick={() => setSelectedArticleId(article.id)}
                  >
                    <span title={article.feed_title}>{shortSourceName(article.feed_title)}</span>
                    <strong>{article.title}</strong>
                  </button>
                ))}
              </section>
            )}

            {showTranslation && selectedArticle.translation?.status === 'done' && (
              <div className="translation-banner" role="status">
                <span>
                  AI 翻译 · {selectedArticle.translation.model} · 机器翻译仅供参考
                </span>
                <button type="button" onClick={() => setShowTranslation(false)}>查看原文</button>
              </div>
            )}

            <SelectionTranslator
              articleId={selectedArticle.id}
              sourceLanguage={selectedArticle.language}
              translated={showTranslation && selectedArticle.translation?.status === 'done'}
            >
              {formatArticleParagraphs(
                showTranslation && selectedArticle.translation?.status === 'done'
                  ? selectedArticle.translation.translated_content || '译文为空。'
                  : selectedArticle.extracted_content || selectedArticle.content || '这篇文章没有可显示的正文。'
              )
                .map((paragraph, index) => (
                  <p key={index}>{paragraph}</p>
                ))}
            </SelectionTranslator>
          </article>
        ) : (
          <div className="reader-empty">
            <Rss size={40} />
            <h2>选择一篇文章开始阅读</h2>
            <p>快捷键：j/k 切换文章，s 生成摘要，f 收藏。</p>
          </div>
        )}
      </main>
      </div>
    </>
  )
}

function ThemeToggle({ theme, onToggleTheme }: { theme: Theme; onToggleTheme: () => void }) {
  const isDark = theme === 'dark'
  return (
    <button
      type="button"
      className="theme-toggle"
      onClick={onToggleTheme}
      aria-label={isDark ? '切换到浅色模式' : '切换到黑夜模式'}
      aria-pressed={isDark}
      title={isDark ? '浅色模式' : '黑夜模式'}
    >
      {isDark ? <Sun size={16} /> : <Moon size={16} />}
    </button>
  )
}

function MonitorPanel({
  monitors,
  busy,
  onSaveMonitor,
  onDeleteMonitor
}: {
  monitors: Monitor[]
  busy: boolean
  onSaveMonitor: (monitorId: string | null, payload: MonitorRequest) => Promise<void>
  onDeleteMonitor: (monitor: Monitor) => Promise<void>
}) {
  const [editingId, setEditingId] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [query, setQuery] = useState('')
  const [minQuality, setMinQuality] = useState(55)
  const [unreadOnly, setUnreadOnly] = useState(false)
  const [enabled, setEnabled] = useState(true)

  function reset() {
    setEditingId(null)
    setName('')
    setQuery('')
    setMinQuality(55)
    setUnreadOnly(false)
    setEnabled(true)
  }

  function edit(monitor: Monitor) {
    setEditingId(monitor.id)
    setName(monitor.name)
    setQuery(monitor.query)
    setMinQuality(monitor.min_quality)
    setUnreadOnly(monitor.unread_only)
    setEnabled(monitor.enabled)
  }

  async function submit(event: FormEvent) {
    event.preventDefault()
    await onSaveMonitor(editingId, {
      name: name.trim(),
      query: query.trim(),
      min_quality: minQuality,
      unread_only: unreadOnly,
      enabled
    })
    reset()
  }

  return (
    <section className="settings-section settings-section-wide monitor-settings">
      <div className="settings-section-header">
        <div>
          <h2>主题监控</h2>
        </div>
        {editingId && <button type="button" className="secondary-button" onClick={reset}>取消编辑</button>}
      </div>
      <form className="monitor-form" onSubmit={submit}>
        <input value={name} onChange={(event) => setName(event.target.value)} placeholder="监控名称，如 AI Agent" aria-label="监控名称" />
        <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="关键词或查询，如 agent protocol" aria-label="监控查询" />
        <label>
          最低质量 {minQuality}
          <input type="range" min="0" max="100" step="5" value={minQuality} onChange={(event) => setMinQuality(Number(event.target.value))} />
        </label>
        <KeywordCheckbox checked={unreadOnly} onChange={setUnreadOnly} label="仅未读" />
        <KeywordCheckbox checked={enabled} onChange={setEnabled} label="启用" />
        <button className="primary-button icon-label-button" disabled={busy || !name.trim() || !query.trim()}>
          <BellRing size={16} /> {editingId ? '保存监控' : '创建监控'}
        </button>
      </form>
      <div className="monitor-list">
        {monitors.length === 0 ? (
          <div className="settings-empty compact">暂无主题监控</div>
        ) : monitors.map((monitor) => (
          <div className="monitor-row" key={monitor.id}>
            <div>
              <strong>{monitor.name}</strong>
              <small>{monitor.query} · 质量 ≥ {monitor.min_quality}{monitor.unread_only ? ' · 仅未读' : ''}</small>
            </div>
            <span>{monitor.enabled ? '启用' : '停用'}</span>
            <button type="button" className="icon-button" onClick={() => edit(monitor)} disabled={busy} aria-label={`编辑 ${monitor.name}`}><Pencil size={16} /></button>
            <button type="button" className="icon-button danger" onClick={() => onDeleteMonitor(monitor)} disabled={busy} aria-label={`删除 ${monitor.name}`}><Trash2 size={16} /></button>
          </div>
        ))}
      </div>
    </section>
  )
}

function BriefEditionsPanel() {
  const [data, setData] = useState<BriefEditionList | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [publishing, setPublishing] = useState(false)

  const load = useCallback(async () => {
    try {
      setData(await api<BriefEditionList>('/api/v2/brief/editions'))
      setError(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : '日报列表读取失败')
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  async function publishToday() {
    setPublishing(true)
    try {
      await api('/api/v2/brief/editions', { method: 'POST' })
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : '生成失败，请稍后重试')
    } finally {
      setPublishing(false)
    }
  }

  return (
    <section className="settings-section settings-section-wide">
      <div className="settings-section-header">
        <div>
          <h2>日报</h2>
          <p>{data?.feed_path ? `订阅地址：${window.location.origin}${data.feed_path}?token=（BRIEF_FEED_TOKEN）` : '订阅未开启：设置 BRIEF_FEED_TOKEN 后可用 RSS 阅读器订阅'}</p>
        </div>
        <button type="button" className="secondary-button" onClick={() => void publishToday()} disabled={publishing}>
          <RefreshCw size={16} /> {publishing ? '生成中…' : '生成今天'}
        </button>
      </div>
      {error && <div className="settings-empty compact danger-text">{error}</div>}
      <div className="monitor-list">
        {data === null && !error ? (
          <div className="settings-empty compact">读取中…</div>
        ) : data && data.editions.length === 0 ? (
          <div className="settings-empty compact">还没有日报，每天定时生成</div>
        ) : data?.editions.map((edition) => (
          <div className="monitor-row" key={edition.edition_date}>
            <div>
              <strong>{edition.edition_date}</strong>
              <small>{edition.item_count} 条 · 生成于 {formatDate(edition.generated_at)}</small>
            </div>
            <a className="secondary-button" href={`/api/v2/brief/editions/${edition.edition_date}?format=markdown`} target="_blank" rel="noreferrer">
              <Download size={16} /> Markdown
            </a>
          </div>
        ))}
      </div>
    </section>
  )
}

function SocialSourcesPanel({
  catalog,
  loading,
  catalogError,
  activityPubHandle,
  rssHubRoute,
  busy,
  onActivityPubHandleChange,
  onRssHubRouteChange,
  onAddActivityPubSource,
  onAddRssHubSource,
  onAddRssHubPresetSource,
  onAddOpenCliSource
}: {
  catalog: SocialSourceCatalog | null
  loading: boolean
  catalogError: string | null
  activityPubHandle: string
  rssHubRoute: string
  busy: boolean
  onActivityPubHandleChange: (value: string) => void
  onRssHubRouteChange: (value: string) => void
  onAddActivityPubSource: (event: FormEvent) => void
  onAddRssHubSource: (event: FormEvent) => void
  onAddRssHubPresetSource: (source: RssHubSocialSource) => void
  onAddOpenCliSource: (source: OpenCliSocialSource) => void
}) {
  return (
    <section className="settings-section settings-section-wide social-source-section">
      <div className="settings-section-header">
        <div>
          <h2>社媒抓取</h2>
        </div>
      </div>

      {loading ? (
        <div className="settings-empty compact" role="status">正在读取可用的社媒适配器…</div>
      ) : catalogError ? (
        <div className="settings-empty compact social-catalog-error" role="alert">{catalogError}</div>
      ) : !catalog ? (
        <div className="settings-empty compact">社媒来源目录未加载，请重新进入设置页后重试。</div>
      ) : (
        <div className="social-source-grid">
          <article className="social-source-card">
            <div className="social-source-card-heading">
              <div className="social-source-title">
                <Send size={18} aria-hidden="true" />
                <div>
                  <h3>ActivityPub</h3>
                </div>
              </div>
              <span className={`social-source-availability${catalog.activitypub_enabled ? ' enabled' : ''}`}>
                {catalog.activitypub_enabled ? '可用' : '未启用'}
              </span>
            </div>
            {catalog.activitypub_enabled ? (
              <form className="settings-inline-form social-source-form" onSubmit={onAddActivityPubSource}>
                <input
                  value={activityPubHandle}
                  onChange={(event) => onActivityPubHandleChange(event.target.value)}
                  placeholder="@user@host"
                  aria-label="ActivityPub 账号"
                  title="请输入 @user@host 格式"
                  pattern="@[^@\s]+@[^@\s]+"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  disabled={busy}
                  required
                />
                <button className="primary-button icon-label-button" disabled={busy || !activityPubHandle.trim()}>
                  <Plus size={16} />
                  测试并添加
                </button>
              </form>
            ) : (
              <div className="settings-empty compact">服务端未启用 ActivityPub 抓取。</div>
            )}
          </article>

          <article className="social-source-card">
            <div className="social-source-card-heading">
              <div className="social-source-title">
                <Rss size={18} aria-hidden="true" />
                <div>
                  <h3>RSSHub route</h3>
                  <p>只填 route</p>
                </div>
              </div>
              <span className={`social-source-availability${catalog.rsshub_enabled ? ' enabled' : ''}`}>
                {catalog.rsshub_enabled ? '可用' : '未配置'}
              </span>
            </div>
            {catalog.rsshub_enabled ? (
              <>
                <form className="settings-inline-form social-source-form" onSubmit={onAddRssHubSource}>
                  <input
                    value={rssHubRoute}
                    onChange={(event) => onRssHubRouteChange(event.target.value)}
                    placeholder="/bilibili/popular/all/1"
                    aria-label="RSSHub route"
                    autoCapitalize="none"
                    autoCorrect="off"
                    spellCheck={false}
                    disabled={busy}
                    required
                  />
                  <button className="primary-button icon-label-button" disabled={busy || !rssHubRoute.trim()}>
                    <Plus size={16} />
                    测试并添加
                  </button>
                </form>
                <div className="opencli-source-list" aria-label="推荐 Telegram 新闻源">
                  {TELEGRAM_NEWS_SOURCES.map((source) => (
                    <div className="opencli-source-item" key={source.path}>
                      <div className="opencli-source-copy">
                        <strong>{source.title}</strong>
                        <small>{source.description}</small>
                      </div>
                      <button
                        type="button"
                        className="secondary-button"
                        onClick={() => onAddRssHubPresetSource(source)}
                        disabled={busy}
                      >
                        <Plus size={16} />
                        测试并添加
                      </button>
                    </div>
                  ))}
                </div>
              </>
            ) : (
              <div className="settings-empty compact">配置 RSSHUB_BASE_URL 后才会开放 route 输入。</div>
            )}
          </article>

          <article className="social-source-card social-source-card-opencli">
            <div className="social-source-card-heading">
              <div className="social-source-title">
                <Sparkles size={18} aria-hidden="true" />
                <div>
                  <h3>OpenCLI 白名单</h3>
                  <p>仅显示服务端白名单</p>
                </div>
              </div>
              <span className={`social-source-availability${catalog.opencli.length > 0 ? ' enabled' : ''}`}>
                {catalog.opencli.length > 0 ? `${catalog.opencli.length} 个` : '未配置'}
              </span>
            </div>
            {catalog.opencli.length > 0 ? (
              <div className="opencli-source-list">
                {catalog.opencli.map((source) => (
                  <div className="opencli-source-item" key={source.name}>
                    <div className="opencli-source-copy">
                      <strong>{source.title}</strong>
                      <small>{source.description ?? `白名单名称：${source.name}`}</small>
                    </div>
                    <button
                      type="button"
                      className="secondary-button"
                      onClick={() => onAddOpenCliSource(source)}
                      disabled={busy}
                    >
                      <Plus size={16} />
                      测试并添加
                    </button>
                  </div>
                ))}
              </div>
            ) : (
              <div className="settings-empty compact">服务端还没有配置 OpenCLI 白名单来源。</div>
            )}
          </article>
        </div>
      )}
    </section>
  )
}

function SettingsPage({
  theme,
  feeds,
  folders,
  groupedFeeds,
  ungroupedFeeds,
  presetGroups,
  keywordRules,
  keywordMatches,
  monitors,
  keywordMatchRuleFilter,
  selectedKeywordMatchIds,
  feedUrl,
  newFeedFolderId,
  folderName,
  opmlText,
  socialSourceCatalog,
  socialCatalogLoading,
  socialCatalogError,
  activityPubHandle,
  rssHubRoute,
  busy,
  notice,
  error,
  opmlFileInputRef,
  onBack,
  onToggleTheme,
  onFeedUrlChange,
  onNewFeedFolderChange,
  onFolderNameChange,
  onOpmlTextChange,
  onActivityPubHandleChange,
  onRssHubRouteChange,
  onAddFeed,
  onAddFolder,
  onAddActivityPubSource,
  onAddRssHubSource,
  onAddRssHubPresetSource,
  onAddOpenCliSource,
  onAddReadyFeed,
  onAddPresetGroup,
  onFetchFeed,
  onDeleteFeed,
  onUpdateSourcePreference,
  onSaveMonitor,
  onDeleteMonitor,
  onSaveKeywordRule,
  onScanKeywordRule,
  onDeleteKeywordRule,
  onKeywordMatchRuleFilterChange,
  onToggleKeywordMatchSelection,
  onSelectVisibleKeywordMatches,
  onRunKeywordMatchAction,
  onImportOpml,
  onLoadOpmlFile,
  onLogout
}: {
  theme: Theme
  feeds: Feed[]
  folders: Folder[]
  groupedFeeds: FeedGroup[]
  ungroupedFeeds: Feed[]
  presetGroups: PresetGroup[]
  keywordRules: KeywordRule[]
  keywordMatches: KeywordMatch[]
  monitors: Monitor[]
  keywordMatchRuleFilter: string
  selectedKeywordMatchIds: string[]
  feedUrl: string
  newFeedFolderId: string
  folderName: string
  opmlText: string
  socialSourceCatalog: SocialSourceCatalog | null
  socialCatalogLoading: boolean
  socialCatalogError: string | null
  activityPubHandle: string
  rssHubRoute: string
  busy: boolean
  notice: string | null
  error: string | null
  opmlFileInputRef: RefObject<HTMLInputElement | null>
  onBack: () => void
  onToggleTheme: () => void
  onFeedUrlChange: (value: string) => void
  onNewFeedFolderChange: (value: string) => void
  onFolderNameChange: (value: string) => void
  onOpmlTextChange: (value: string) => void
  onActivityPubHandleChange: (value: string) => void
  onRssHubRouteChange: (value: string) => void
  onAddFeed: (event: FormEvent) => void
  onAddFolder: (event: FormEvent) => void
  onAddActivityPubSource: (event: FormEvent) => void
  onAddRssHubSource: (event: FormEvent) => void
  onAddRssHubPresetSource: (source: RssHubSocialSource) => void
  onAddOpenCliSource: (source: OpenCliSocialSource) => void
  onAddReadyFeed: (preset: ReadyMadeFeed) => void
  onAddPresetGroup: (category: string) => void
  onFetchFeed: (feedId: string, title?: string) => void
  onDeleteFeed: (feed: Feed) => void
  onUpdateSourcePreference: (feed: Feed, patch: FeedPreferencePatch) => void
  onSaveMonitor: (monitorId: string | null, payload: MonitorRequest) => Promise<void>
  onDeleteMonitor: (monitor: Monitor) => Promise<void>
  onSaveKeywordRule: (ruleId: string | null, payload: KeywordRuleRequest) => Promise<void>
  onScanKeywordRule: (rule: KeywordRule) => Promise<void>
  onDeleteKeywordRule: (rule: KeywordRule) => Promise<void>
  onKeywordMatchRuleFilterChange: (ruleId: string) => void
  onToggleKeywordMatchSelection: (matchId: string) => void
  onSelectVisibleKeywordMatches: () => void
  onRunKeywordMatchAction: (summarize: boolean, telegram: boolean) => Promise<void>
  onImportOpml: () => void
  onLoadOpmlFile: (event: ChangeEvent<HTMLInputElement>) => void
  onLogout: () => void
}) {
  const [sourceQuery, setSourceQuery] = useState('')
  const [sourceStatus, setSourceStatus] = useState<SourceStatus>('all')
  const [presetQuery, setPresetQuery] = useState('')
  const visibleFeeds = filterSources(feeds, sourceQuery, sourceStatus)
  const visibleFeedIds = new Set(visibleFeeds.map((feed) => feed.id))
  const visibleUngrouped = ungroupedFeeds.filter((feed) => visibleFeedIds.has(feed.id))
  const visibleGroups = groupedFeeds.map((group) => ({ ...group, feeds: group.feeds.filter((feed) => visibleFeedIds.has(feed.id)) })).filter((group) => group.feeds.length > 0)
  const presetTerms = presetQuery.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  const visiblePresets = presetGroups.map((group) => ({ ...group, feeds: group.feeds.filter((preset) => presetTerms.every((term) => `${preset.title} ${preset.category} ${presetDisplayUrl(preset)}`.toLocaleLowerCase().includes(term))) })).filter((group) => group.feeds.length > 0)
  const totalUnread = feeds.reduce((total, feed) => total + feed.unread_count, 0)

  return (
    <>
      <a className="skip-link" href="#main-content">跳到设置内容</a>
      <main id="main-content" className="settings-shell" tabIndex={-1}>
      <header className="settings-header">
        <div className="brand-line">
          <Settings size={28} />
          <div>
            <h1>设置</h1>
          </div>
        </div>
        <div className="settings-header-actions">
          <ThemeToggle theme={theme} onToggleTheme={onToggleTheme} />
          <button type="button" className="secondary-button" onClick={onLogout}>
            <LogOut size={16} />
            退出登录
          </button>
          <button type="button" className="secondary-button" onClick={onBack}>
            <ArrowLeft size={16} />
            返回报告
          </button>
        </div>
      </header>

      {(notice || error) && (
        <div
          className={error ? 'settings-status error' : 'settings-status'}
          role={error ? 'alert' : 'status'}
          aria-live={error ? 'assertive' : 'polite'}
        >
          {error ?? notice}
        </div>
      )}

      <div className="settings-layout">
        <section className="settings-section settings-section-wide">
          <div className="settings-section-header">
            <div>
              <h2>添加订阅</h2>
            </div>
          </div>
          <form className="settings-inline-form feed-settings-form" onSubmit={onAddFeed}>
            <input
              value={feedUrl}
              onChange={(event) => onFeedUrlChange(event.target.value)}
              placeholder="https://example.com/feed.xml"
              aria-label="订阅地址"
            />
            <select
              value={newFeedFolderId}
              onChange={(event) => onNewFeedFolderChange(event.target.value)}
              disabled={busy}
              aria-label="订阅分类"
            >
              <option value="">未分类</option>
              {folders.map((folder) => (
                <option key={folder.id} value={folder.id}>
                  {folder.title}
                </option>
              ))}
            </select>
            <button className="primary-button icon-label-button" disabled={busy || !feedUrl.trim()}>
              <Plus size={16} />
              添加
            </button>
          </form>
          <form className="settings-inline-form folder-settings-form" onSubmit={onAddFolder}>
            <input
              value={folderName}
              onChange={(event) => onFolderNameChange(event.target.value)}
              placeholder="新分类名称"
              aria-label="新分类名称"
            />
            <button className="secondary-button" disabled={busy || !folderName.trim()}>
              <FolderPlus size={16} />
              创建分类
            </button>
          </form>
        </section>

        <SocialSourcesPanel
          catalog={socialSourceCatalog}
          loading={socialCatalogLoading}
          catalogError={socialCatalogError}
          activityPubHandle={activityPubHandle}
          rssHubRoute={rssHubRoute}
          busy={busy}
          onActivityPubHandleChange={onActivityPubHandleChange}
          onRssHubRouteChange={onRssHubRouteChange}
          onAddActivityPubSource={onAddActivityPubSource}
          onAddRssHubSource={onAddRssHubSource}
          onAddRssHubPresetSource={onAddRssHubPresetSource}
          onAddOpenCliSource={onAddOpenCliSource}
        />

        <KeywordRulePanel
          feeds={feeds}
          folders={folders}
          keywordRules={keywordRules}
          busy={busy}
          onSaveKeywordRule={onSaveKeywordRule}
          onScanKeywordRule={onScanKeywordRule}
          onDeleteKeywordRule={onDeleteKeywordRule}
        />

        <KeywordMatchesPanel
          keywordRules={keywordRules}
          keywordMatches={keywordMatches}
          keywordMatchRuleFilter={keywordMatchRuleFilter}
          selectedKeywordMatchIds={selectedKeywordMatchIds}
          busy={busy}
          onRuleFilterChange={onKeywordMatchRuleFilterChange}
          onToggleSelection={onToggleKeywordMatchSelection}
          onSelectVisible={onSelectVisibleKeywordMatches}
          onRunAction={onRunKeywordMatchAction}
        />

        <MonitorPanel
          monitors={monitors}
          busy={busy}
          onSaveMonitor={onSaveMonitor}
          onDeleteMonitor={onDeleteMonitor}
        />

        <section className="settings-section settings-section-wide">
          <div className="settings-section-header">
            <div>
              <h2>当前订阅</h2>
              <p>{feeds.length} 个订阅 · {totalUnread} 未读</p>
            </div>
          </div>
          <div className="source-filter-controls">
            <label>搜索订阅<input type="search" value={sourceQuery} onChange={(event) => setSourceQuery(event.target.value)} placeholder="名称或网址" /></label>
            <label>采集状态<select aria-label="采集状态" value={sourceStatus} onChange={(event) => setSourceStatus(event.target.value as SourceStatus)}>
              <option value="all">全部订阅</option><option value="failing">采集异常</option><option value="unverified">尚未采集成功</option><option value="muted">已静音</option><option value="active">未静音</option>
            </select></label>
            {(sourceQuery || sourceStatus !== 'all') && <button type="button" className="secondary-button" onClick={() => { setSourceQuery(''); setSourceStatus('all') }}>清除筛选</button>}
          </div>
          <p role="status">显示 {visibleFeeds.length} / {feeds.length} 个订阅</p>
          <div className="settings-feed-list">
            {visibleFeeds.length === 0 ? (
              <div className="settings-empty">{feeds.length ? '没有符合筛选的订阅' : '暂无订阅'}</div>
            ) : (
              <>
                {visibleUngrouped.length > 0 && (
                  <div className="settings-feed-group">
                    <h3>未分类</h3>
                    {visibleUngrouped.map((feed) => (
                      <SettingsFeedItem
                        key={feed.id}
                        feed={feed}
                        busy={busy}
                        onFetchFeed={onFetchFeed}
                        onDeleteFeed={onDeleteFeed}
                        onUpdateSourcePreference={onUpdateSourcePreference}
                      />
                    ))}
                  </div>
                )}
                {visibleGroups.map((group) => (
                  <div className="settings-feed-group" key={group.folder.id}>
                    <h3>{group.folder.title}</h3>
                    {group.feeds.length === 0 ? (
                      <div className="settings-empty compact">暂无订阅</div>
                    ) : (
                      group.feeds.map((feed) => (
                        <SettingsFeedItem
                          key={feed.id}
                          feed={feed}
                          busy={busy}
                          onFetchFeed={onFetchFeed}
                          onDeleteFeed={onDeleteFeed}
                          onUpdateSourcePreference={onUpdateSourcePreference}
                        />
                      ))
                    )}
                  </div>
                ))}
              </>
            )}
          </div>
        </section>

        <section className="settings-section settings-section-wide">
          <div className="settings-section-header">
            <div>
              <h2>推荐订阅</h2>
            </div>
          </div>
          <div className="source-filter-controls"><label>搜索推荐源<input type="search" value={presetQuery} onChange={(event) => setPresetQuery(event.target.value)} placeholder="名称、分类或网址" /></label></div>
          {visiblePresets.length === 0 && <p role="status">没有匹配的推荐源</p>}
          <div className="settings-preset-grid">
            {visiblePresets.map((group) => (
              <div className="preset-group" key={group.category}>
                <div className="preset-group-title">
                  <span>{group.category}</span>
                  <button type="button" onClick={() => onAddPresetGroup(group.category)} disabled={busy || presetTerms.length > 0} title={presetTerms.length ? '清除搜索后可添加整组' : undefined}>
                    <Plus size={13} />
                    全部
                  </button>
                </div>
                {group.feeds.map((preset) => {
                  const added = feeds.some((feed) => matchesPresetFeed(feed, preset))
                  const unavailable = Boolean(preset.rsshubPath && !socialSourceCatalog?.rsshub_enabled)
                  const displayUrl = presetDisplayUrl(preset)
                  return (
                    <button
                      key={preset.rsshubPath ?? preset.url}
                      type="button"
                      className={added ? 'preset-row added' : 'preset-row'}
                      onClick={() => onAddReadyFeed(preset)}
                      disabled={busy || unavailable || added}
                      title={unavailable ? 'RSSHub 尚未启用' : displayUrl}
                    >
                      <span className="preset-copy">
                        <strong>{preset.title}</strong>
                        <small>{displayUrl}</small>
                      </span>
                      {added ? <CheckCircle2 size={15} /> : <Plus size={15} />}
                    </button>
                  )
                })}
              </div>
            ))}
          </div>
        </section>

        <BriefEditionsPanel />

        <section className="settings-section settings-section-wide">
          <div className="settings-section-header">
            <div>
              <h2>OPML 导入导出</h2>
              <p>保留分类结构</p>
            </div>
          </div>
          <textarea
            className="settings-opml-textarea"
            value={opmlText}
            onChange={(event) => onOpmlTextChange(event.target.value)}
            placeholder="粘贴 OPML XML，或选择 .opml 文件"
            aria-label="OPML 内容"
          />
          <input
            ref={opmlFileInputRef}
            className="visually-hidden"
            type="file"
            hidden
            accept=".opml,.xml,text/xml,application/xml"
            onChange={onLoadOpmlFile}
          />
          <div className="settings-action-row">
            <button type="button" className="secondary-button" onClick={() => opmlFileInputRef.current?.click()} disabled={busy}>
              <FileUp size={16} />
              选择文件
            </button>
            <button type="button" className="primary-button icon-label-button" onClick={onImportOpml} disabled={busy || !opmlText.trim()}>
              <Import size={16} />
              导入
            </button>
            <a className="secondary-button" href="/api/opml/export">
              <Download size={16} />
              导出
            </a>
          </div>
        </section>
      </div>
      </main>
    </>
  )
}

function KeywordRulePanel({
  feeds,
  folders,
  keywordRules,
  busy,
  onSaveKeywordRule,
  onScanKeywordRule,
  onDeleteKeywordRule
}: {
  feeds: Feed[]
  folders: Folder[]
  keywordRules: KeywordRule[]
  busy: boolean
  onSaveKeywordRule: (ruleId: string | null, payload: KeywordRuleRequest) => Promise<void>
  onScanKeywordRule: (rule: KeywordRule) => Promise<void>
  onDeleteKeywordRule: (rule: KeywordRule) => Promise<void>
}) {
  const [editingRuleId, setEditingRuleId] = useState<string | null>(null)
  const [name, setName] = useState('')
  const [keywordsText, setKeywordsText] = useState('')
  const [matchTitle, setMatchTitle] = useState(true)
  const [matchContent, setMatchContent] = useState(true)
  const [caseSensitive, setCaseSensitive] = useState(false)
  const [autoSummary, setAutoSummary] = useState(true)
  const [telegramEnabled, setTelegramEnabled] = useState(false)
  const [enabled, setEnabled] = useState(true)
  const [selectedFeedIds, setSelectedFeedIds] = useState<string[]>([])
  const [selectedFolderIds, setSelectedFolderIds] = useState<string[]>([])

  const keywords = useMemo(() => splitKeywords(keywordsText), [keywordsText])

  function resetForm() {
    setEditingRuleId(null)
    setName('')
    setKeywordsText('')
    setMatchTitle(true)
    setMatchContent(true)
    setCaseSensitive(false)
    setAutoSummary(true)
    setTelegramEnabled(false)
    setEnabled(true)
    setSelectedFeedIds([])
    setSelectedFolderIds([])
  }

  function editRule(rule: KeywordRule) {
    setEditingRuleId(rule.id)
    setName(rule.name)
    setKeywordsText(rule.keywords.join('\n'))
    setMatchTitle(rule.match_title)
    setMatchContent(rule.match_content)
    setCaseSensitive(rule.case_sensitive)
    setAutoSummary(rule.auto_summary)
    setTelegramEnabled(rule.telegram_enabled)
    setEnabled(rule.enabled)
    setSelectedFeedIds(rule.feed_ids)
    setSelectedFolderIds(rule.folder_ids)
  }

  async function submitRule(event: FormEvent) {
    event.preventDefault()
    await onSaveKeywordRule(editingRuleId, {
      name,
      keywords,
      match_title: matchTitle,
      match_content: matchContent,
      case_sensitive: caseSensitive,
      auto_summary: autoSummary,
      telegram_enabled: telegramEnabled,
      enabled,
      feed_ids: selectedFeedIds,
      folder_ids: selectedFolderIds
    })
    resetForm()
  }

  return (
    <section className="settings-section settings-section-wide keyword-section">
      <div className="settings-section-header">
        <div>
          <h2>关键词检测</h2>
          <p>{keywordRules.length} 条规则</p>
        </div>
        {editingRuleId && (
          <button type="button" className="secondary-button" onClick={resetForm}>
            取消编辑
          </button>
        )}
      </div>

      <form className="keyword-rule-form" onSubmit={submitRule}>
        <div className="keyword-form-main">
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="规则名称"
            aria-label="关键词规则名称"
          />
          <textarea
            value={keywordsText}
            onChange={(event) => setKeywordsText(event.target.value)}
            placeholder="关键词，每行或逗号分隔"
            aria-label="关键词列表"
          />
        </div>

        <div className="keyword-toggle-grid">
          <KeywordCheckbox checked={matchTitle} onChange={setMatchTitle} label="标题" />
          <KeywordCheckbox checked={matchContent} onChange={setMatchContent} label="正文" />
          <KeywordCheckbox checked={caseSensitive} onChange={setCaseSensitive} label="区分大小写" />
          <KeywordCheckbox checked={autoSummary} onChange={setAutoSummary} label="自动 AI 总结" />
          <KeywordCheckbox checked={telegramEnabled} onChange={setTelegramEnabled} label="自动 TG 推送" />
          <KeywordCheckbox checked={enabled} onChange={setEnabled} label="启用" />
        </div>

        <div className="keyword-target-panel">
          <div className="keyword-target-actions">
            <span>检测目标</span>
            <button type="button" onClick={() => setSelectedFolderIds(folders.map((folder) => folder.id))}>
              全选分类
            </button>
            <button type="button" onClick={() => setSelectedFeedIds(feeds.map((feed) => feed.id))}>
              全选订阅
            </button>
            <button type="button" onClick={() => { setSelectedFolderIds([]); setSelectedFeedIds([]) }}>
              全部订阅
            </button>
          </div>
          <div className="keyword-target-grid">
            {folders.map((folder) => (
              <KeywordCheckbox
                key={folder.id}
                checked={selectedFolderIds.includes(folder.id)}
                onChange={(checked) => setSelectedFolderIds(toggleId(selectedFolderIds, folder.id, checked))}
                label={`分类 / ${folder.title}`}
              />
            ))}
            {feeds.map((feed) => (
              <KeywordCheckbox
                key={feed.id}
                checked={selectedFeedIds.includes(feed.id)}
                onChange={(checked) => setSelectedFeedIds(toggleId(selectedFeedIds, feed.id, checked))}
                label={feed.title}
              />
            ))}
          </div>
        </div>

        <button
          className="primary-button icon-label-button keyword-submit"
          disabled={busy || !name.trim() || keywords.length === 0 || (!matchTitle && !matchContent)}
        >
          <Tags size={16} />
          {editingRuleId ? '保存规则' : '创建规则'}
        </button>
      </form>

      <div className="keyword-rule-list">
        {keywordRules.length === 0 ? (
          <div className="settings-empty compact">暂无规则</div>
        ) : (
          keywordRules.map((rule) => (
            <div className="keyword-rule-card" key={rule.id}>
              <div className="keyword-rule-copy">
                <div className="keyword-rule-title">
                  <strong>{rule.name}</strong>
                  <span>{rule.enabled ? '启用' : '停用'}</span>
                </div>
                <div className="keyword-chip-row">
                  {rule.keywords.map((keyword) => (
                    <span key={keyword}>{keyword}</span>
                  ))}
                </div>
                <small>{keywordTargetSummary(rule)} · {rule.match_count} 条命中</small>
              </div>
              <div className="settings-feed-actions">
                <button type="button" className="icon-button" onClick={() => editRule(rule)} disabled={busy} title="编辑规则" aria-label={`编辑 ${rule.name}`}>
                  <Pencil size={16} />
                </button>
                <button type="button" className="icon-button" onClick={() => onScanKeywordRule(rule)} disabled={busy} title="扫描规则" aria-label={`扫描 ${rule.name}`}>
                  <RefreshCw size={16} />
                </button>
                <button type="button" className="icon-button danger" onClick={() => onDeleteKeywordRule(rule)} disabled={busy} title="删除规则" aria-label={`删除 ${rule.name}`}>
                  <Trash2 size={16} />
                </button>
              </div>
            </div>
          ))
        )}
      </div>
    </section>
  )
}

function KeywordMatchesPanel({
  keywordRules,
  keywordMatches,
  keywordMatchRuleFilter,
  selectedKeywordMatchIds,
  busy,
  onRuleFilterChange,
  onToggleSelection,
  onSelectVisible,
  onRunAction
}: {
  keywordRules: KeywordRule[]
  keywordMatches: KeywordMatch[]
  keywordMatchRuleFilter: string
  selectedKeywordMatchIds: string[]
  busy: boolean
  onRuleFilterChange: (ruleId: string) => void
  onToggleSelection: (matchId: string) => void
  onSelectVisible: () => void
  onRunAction: (summarize: boolean, telegram: boolean) => Promise<void>
}) {
  return (
    <section className="settings-section settings-section-wide keyword-section">
      <div className="settings-section-header">
        <div>
          <h2>命中结果</h2>
          <p>{keywordMatches.length} 条 · 已选 {selectedKeywordMatchIds.length}</p>
        </div>
        <div className="keyword-match-toolbar">
          <select
            value={keywordMatchRuleFilter}
            onChange={(event) => onRuleFilterChange(event.target.value)}
            aria-label="按关键词规则筛选命中结果"
          >
            <option value="">全部规则</option>
            {keywordRules.map((rule) => (
              <option key={rule.id} value={rule.id}>
                {rule.name}
              </option>
            ))}
          </select>
          <button type="button" className="secondary-button" onClick={onSelectVisible} disabled={busy || keywordMatches.length === 0}>
            <ListChecks size={16} />
            全选当前
          </button>
          <button type="button" className="secondary-button" onClick={() => onRunAction(true, false)} disabled={busy || selectedKeywordMatchIds.length === 0}>
            <Sparkles size={16} />
            AI 总结
          </button>
          <button type="button" className="secondary-button" onClick={() => onRunAction(false, true)} disabled={busy || selectedKeywordMatchIds.length === 0}>
            <Send size={16} />
            TG 推送
          </button>
          <button type="button" className="primary-button icon-label-button" onClick={() => onRunAction(true, true)} disabled={busy || selectedKeywordMatchIds.length === 0}>
            <BellRing size={16} />
            总结并推送
          </button>
        </div>
      </div>

      <div className="keyword-match-list">
        {keywordMatches.length === 0 ? (
          <div className="settings-empty compact">暂无命中</div>
        ) : (
          keywordMatches.map((match) => (
            <label className="keyword-match-row" key={match.id}>
              <input
                type="checkbox"
                checked={selectedKeywordMatchIds.includes(match.id)}
                onChange={() => onToggleSelection(match.id)}
              />
              <span className="keyword-match-copy">
                <strong>{match.article_title}</strong>
                <small>{match.rule_name} · {match.feed_title} · {formatDate(match.published_at ?? match.fetched_at)}</small>
                <span>{match.excerpt}</span>
                <span className="keyword-chip-row">
                  {match.matched_keywords.map((keyword) => (
                    <b key={keyword}>{keyword}</b>
                  ))}
                </span>
              </span>
              <span className="keyword-match-status">
                <b>{match.summary_status ? `摘要 ${match.summary_status}` : '未总结'}</b>
                <b className={match.telegram_status === 'failed' ? 'danger-text' : ''}>
                  {telegramStatusLabel(match.telegram_status)}
                </b>
              </span>
            </label>
          ))
        )}
      </div>
    </section>
  )
}

function KeywordCheckbox({
  checked,
  onChange,
  label
}: {
  checked: boolean
  onChange: (checked: boolean) => void
  label: string
}) {
  return (
    <label className="keyword-check">
      <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
      <span>{label}</span>
    </label>
  )
}

function splitKeywords(value: string): string[] {
  const keywords: string[] = []
  for (const keyword of value.split(/[\n\r,，;；]/).map((item) => item.trim()).filter(Boolean)) {
    if (!keywords.some((item) => item.toLowerCase() === keyword.toLowerCase())) {
      keywords.push(keyword)
    }
  }
  return keywords
}

function toggleId(values: string[], id: string, checked: boolean): string[] {
  if (checked) return values.includes(id) ? values : [...values, id]
  return values.filter((value) => value !== id)
}

function keywordTargetSummary(rule: KeywordRule): string {
  if (rule.feed_ids.length === 0 && rule.folder_ids.length === 0) return '全部订阅'
  return `${rule.folder_ids.length} 个分类 · ${rule.feed_ids.length} 个订阅`
}

function telegramStatusLabel(status: KeywordMatch['telegram_status']): string {
  if (status === 'sent') return 'TG 已发'
  if (status === 'failed') return 'TG 失败'
  return 'TG 未发'
}

function FeedRow({
  feed,
  active,
  onClick,
  nested = false
}: {
  feed: Feed
  active: boolean
  onClick: (id: string) => void
  nested?: boolean
}) {
  return (
    <button
      className={`${active ? 'feed-row active' : 'feed-row'}${nested ? ' nested' : ''}`}
      onClick={() => onClick(feed.id)}
      title={feed.feed_url}
      aria-pressed={active}
    >
      <span>{feed.title}</span>
      <b>{feed.unread_count}</b>
    </button>
  )
}

function FolderRow({
  folder,
  active,
  unreadCount,
  onClick
}: {
  folder: Folder
  active: boolean
  unreadCount: number
  onClick: (id: string) => void
}) {
  return (
    <button
      className={active ? 'folder-row active' : 'folder-row'}
      onClick={() => onClick(folder.id)}
      aria-pressed={active}
    >
      <span>{folder.title}</span>
      <b>{unreadCount}</b>
    </button>
  )
}

function SettingsFeedItem({
  feed,
  busy,
  onFetchFeed,
  onDeleteFeed,
  onUpdateSourcePreference
}: {
  feed: Feed
  busy: boolean
  onFetchFeed: (feedId: string, title?: string) => void
  onDeleteFeed: (feed: Feed) => void
  onUpdateSourcePreference: (feed: Feed, patch: FeedPreferencePatch) => void
}) {
  return (
    <div className="settings-feed-item">
      <div className="settings-feed-copy">
        <strong>{feed.title}</strong>
        <small>{feed.feed_url}</small>
        <div className="settings-feed-meta">
          <span>{feed.article_count} 篇文章</span>
          <span>{feed.unread_count} 未读</span>
          <span>来源质量 {feed.quality_score}</span>
          {feed.feedback_weight !== 0 && <span>偏好 {feed.feedback_weight > 0 ? '+' : ''}{feed.feedback_weight}</span>}
          {feed.quality_stats?.snr != null && (
            <span title="近 30 天评审达标的事件数 / 文章数">信噪比 {Math.round(feed.quality_stats.snr * 100)}%</span>
          )}
          {feed.quality_stats?.average_score != null && <span>评审均分 {feed.quality_stats.average_score.toFixed(1)}</span>}
          {!!feed.quality_stats?.auto_adjust && (
            <span title="按信噪比自动调整排序用的来源质量">排序微调 {feed.quality_stats.auto_adjust > 0 ? '+' : ''}{feed.quality_stats.auto_adjust}</span>
          )}
          {feed.quality_stats?.declining && <span className="danger-text">信噪比较上月减半</span>}
          <span>{feed.last_fetched_at ? `上次抓取 ${formatDate(feed.last_fetched_at)}` : '尚未抓取'}</span>
          {feed.consecutive_failures > 0 && <span className="danger-text">连续失败 {feed.consecutive_failures} 次</span>}
          {feed.muted && <span className="danger-text">已静音</span>}
          {feed.last_error && (
            <span className="danger-text feed-error-text">抓取错误：{feed.last_error}</span>
          )}
        </div>
      </div>
      <div className="settings-feed-actions">
        <select
          className="settings-feed-profile"
          value={feed.editorial_profile ?? 'auto'}
          onChange={(event) => onUpdateSourcePreference(feed, { editorial_profile: event.target.value as EditorialProfile | 'auto' })}
          disabled={busy}
          aria-label={`${feed.title} 的内容类型`}
          title="评审按内容类型选择评分标准"
        >
          <option value="auto">自动判断</option>
          {(Object.keys(EDITORIAL_PROFILE_LABELS) as EditorialProfile[]).map((profile) => (
            <option key={profile} value={profile}>{EDITORIAL_PROFILE_LABELS[profile]}</option>
          ))}
        </select>
        <button
          type="button"
          className="icon-button"
          onClick={() => onUpdateSourcePreference(feed, { feedback_weight: Math.min(50, feed.feedback_weight + 10) })}
          disabled={busy}
          title="提高来源优先级"
          aria-label={`提高 ${feed.title} 的优先级`}
        >
          <ArrowUp size={16} />
        </button>
        <button
          type="button"
          className="icon-button"
          onClick={() => onUpdateSourcePreference(feed, { feedback_weight: Math.max(-50, feed.feedback_weight - 10) })}
          disabled={busy}
          title="降低来源优先级"
          aria-label={`降低 ${feed.title} 的优先级`}
        >
          <ArrowDown size={16} />
        </button>
        <button
          type="button"
          className={feed.muted ? 'icon-button active' : 'icon-button'}
          onClick={() => onUpdateSourcePreference(feed, { muted: !feed.muted })}
          disabled={busy}
          title={feed.muted ? '恢复来源' : '静音来源'}
          aria-label={`${feed.muted ? '恢复' : '静音'} ${feed.title}`}
        >
          <VolumeX size={16} />
        </button>
        <button
          type="button"
          className="icon-button"
          onClick={() => onFetchFeed(feed.id, feed.title)}
          disabled={busy}
          title="重新抓取"
          aria-label={`重新抓取 ${feed.title}`}
        >
          <RefreshCw size={16} />
        </button>
        <button
          type="button"
          className="icon-button danger"
          onClick={() => onDeleteFeed(feed)}
          disabled={busy}
          title="删除订阅"
          aria-label={`删除 ${feed.title}`}
        >
          <Trash2 size={16} />
        </button>
      </div>
    </div>
  )
}

function SummaryPanel({ article }: { article: ArticleDetail }) {
  if (!article.summary) {
    return (
      <section className="summary-panel muted">
        <Sparkles size={18} />
        <span>还没有摘要。点击右上角闪光按钮生成。</span>
      </section>
    )
  }
  const summary = article.summary
  const verifiedCitations = summary.citations.filter((citation) => citation.verified)
  const unverifiedCitations = summary.citations.filter((citation) => !citation.verified)
  const isExtractive = summary.method.startsWith('extractive-')
  return (
    <section className={summary.status === 'failed' ? 'summary-panel failed' : 'summary-panel'}>
      <div className="summary-title">
        <Sparkles size={18} />
        <strong>信息摘要</strong>
        <span>{summaryMethodLabel(summary.method)}{summary.model ? ` · ${summary.model}` : ''}</span>
      </div>
      {summary.status === 'done' ? (
        <>
          <div className="summary-trust">
            <strong>
              材料支撑度：{summary.grounding_score === null ? '未评估' : `${summary.grounding_score}/100`}
            </strong>
            <span>
              {corroborationLabel(
                summary.corroboration,
                summary.source_count,
                summary.cited_source_count
              )}
            </span>
            {summary.confidence_reason && <span>{summary.confidence_reason}</span>}
          </div>
          <p>{summary.summary}</p>
          {summary.bullets.length > 0 && (
            <ul>
              {summary.bullets.map((bullet) => (
                <li key={bullet}>{bullet}</li>
              ))}
            </ul>
          )}
          {summary.disagreements.length > 0 && (
            <div className="summary-citations">
              <strong>来源分歧</strong>
              {summary.disagreements.map((item) => (
                <blockquote key={item}>{item}</blockquote>
              ))}
            </div>
          )}
          {summary.keywords.length > 0 && (
            <div className="keyword-row">
              {summary.keywords.map((keyword) => (
                <span key={keyword}>{keyword}</span>
              ))}
            </div>
          )}
          {verifiedCitations.length > 0 && (
            <div className="summary-citations">
              <strong>已核验依据片段</strong>
              {verifiedCitations.map((citation, index) => (
                <blockquote key={`${citation.evidence_id ?? citation.excerpt}-${index}`}>
                  {citation.url ? (
                    <a href={citation.url} target="_blank" rel="noreferrer">
                      {citationClaimLabel(citation.claim_id)} ·{' '}
                      {citation.source_title ?? citation.article_title ?? '原文来源'}
                    </a>
                  ) : citation.source_title ? (
                    <b>{citationClaimLabel(citation.claim_id)} · {citation.source_title}</b>
                  ) : null}
                  <span>{citation.excerpt}</span>
                </blockquote>
              ))}
            </div>
          )}
          {unverifiedCitations.length > 0 && (
            <div className="summary-citations">
              <strong>{isExtractive ? '本地原文片段（未运行二次核验）' : '旧版引用（未做证据校验）'}</strong>
              {unverifiedCitations.map((citation, index) => (
                <blockquote key={`${citation.excerpt}-${index}`}>
                  <span>{citation.excerpt}</span>
                </blockquote>
              ))}
            </div>
          )}
        </>
      ) : (
        <p>{summary.error ?? `摘要任务状态：${summary.status}`}</p>
      )}
    </section>
  )
}

function summaryMethodLabel(method: string): string {
  if (method.startsWith('grounded-event-')) return '证据核验 · 事件综合'
  if (method.startsWith('grounded-article-')) return '证据核验 · 单篇'
  if (method.startsWith('extractive-')) return '本地原文抽取'
  return method === 'legacy' ? '旧版摘要' : method
}

function citationClaimLabel(claimId: string | null): string {
  if (claimId === 'summary') return '核心摘要'
  if (claimId?.startsWith('bullet:')) return `要点 ${Number(claimId.split(':')[1]) + 1}`
  if (claimId?.startsWith('disagreement:')) return `分歧 ${Number(claimId.split(':')[1]) + 1}`
  return '依据'
}

function corroborationLabel(
  value: Summary['corroboration'],
  sourceCount: number,
  citedSourceCount: number
): string {
  if (value === 'divergent') {
    return `分析 ${sourceCount} 个来源，最终引用 ${citedSourceCount} 个；检测到已核验分歧`
  }
  if (value === 'multi') {
    return `分析 ${sourceCount} 个来源，最终引用 ${citedSourceCount} 个；至少一项主张获多源支撑`
  }
  return sourceCount > 1
    ? `分析 ${sourceCount} 个来源，最终引用 ${Math.max(1, citedSourceCount)} 个；未形成多源共证`
    : '单一来源材料'
}
