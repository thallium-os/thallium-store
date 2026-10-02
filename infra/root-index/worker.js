// R2 custom domains serve objects by exact key and have no index document,
// so "/" is a 404. This answers only the bare root of each store domain with
// that bucket's index.html; every other path goes straight to R2.
export default {
  async fetch(request, env) {
    const host = new URL(request.url).hostname;
    const bucket = host.startsWith("store-qt.") ? env.STORE_QT : env.STORE_QS;
    const page = await bucket.get("index.html");
    if (!page) return new Response("Not found", { status: 404 });
    return new Response(page.body, {
      headers: {
        "content-type": "text/html; charset=utf-8",
        "cache-control": "public, max-age=300",
        etag: page.httpEtag,
      },
    });
  },
};
