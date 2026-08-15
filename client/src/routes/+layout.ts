// Everything here is live and client-side: there is nothing to render on a
// server, and prerendering a view of aircraft would bake in a snapshot that is
// wrong by the time it loads.
export const ssr = false;
export const prerender = false;
