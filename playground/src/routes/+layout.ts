// djls runs in the browser, so there is nothing to render on a server: the build is one
// prerendered shell, served as static files.
export const ssr = false;
export const prerender = true;
