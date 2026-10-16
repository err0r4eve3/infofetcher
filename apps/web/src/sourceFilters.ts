import type { Feed } from './api'

export type SourceStatus = 'all' | 'failing' | 'unverified' | 'muted' | 'active'

export function filterSources(feeds: Feed[], query: string, status: SourceStatus): Feed[] {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  return feeds.filter((feed) => {
    const text = [feed.title, feed.feed_url, feed.site_url, feed.hub_path].filter(Boolean).join(' ').toLocaleLowerCase()
    if (!terms.every((term) => text.includes(term))) return false
    if (status === 'failing') return feed.consecutive_failures > 0 || Boolean(feed.last_error)
    if (status === 'unverified') return !feed.last_success_at
    if (status === 'muted') return feed.muted
    if (status === 'active') return !feed.muted
    return true
  })
}
