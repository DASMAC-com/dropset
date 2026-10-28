/** @type {import('next').NextConfig} */
const nextConfig = {
  // Next 15's dev-tools badge sits on the page for the whole dev session, and
  // `pnpm run export` screenshots a `next dev` server — so left on, it lands in
  // the corner of every exported slide.
  devIndicators: false,
};

export default nextConfig;
