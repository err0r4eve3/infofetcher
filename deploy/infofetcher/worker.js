export default {
  async fetch(request) {
    const upstream = new URL(request.url)
    upstream.hostname = 'error.moe'
    upstream.protocol = 'https:'
    upstream.port = ''
    try {
      return await fetch(new Request(upstream, request), { redirect: 'manual' })
    } catch {
      return new Response('信息服务暂时不可用，请稍后重试。', { status: 502 })
    }
  }
}
