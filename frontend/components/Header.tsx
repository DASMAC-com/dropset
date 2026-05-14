import Image from "next/image";
import Link from "next/link";

export function Header() {
  return (
    <header className="sticky top-0 z-50 border-border border-b bg-background">
      <div className="mx-auto flex h-14 max-w-6xl items-center gap-2 px-6">
        <Link
          href="/"
          className="flex shrink-0 items-center gap-2 font-semibold text-base text-foreground no-underline"
        >
          <Image
            src="/favicon.png"
            alt=""
            width={36}
            height={36}
            priority
            suppressHydrationWarning
          />
          Dropset
        </Link>
        <nav className="flex items-center gap-2">
          <span className="rounded-md bg-muted px-3 py-1.5 text-sm text-foreground">
            Swap
          </span>
        </nav>
        <a
          href="https://x.com/__Dropset__"
          target="_blank"
          rel="noopener noreferrer"
          className="ml-auto inline-flex h-9 w-9 items-center justify-center rounded-md text-foreground hover:bg-muted"
        >
          <span className="sr-only">Dropset on X</span>
          <svg
            viewBox="0 0 24 24"
            className="h-4 w-4"
            fill="currentColor"
            aria-hidden="true"
          >
            <title>X</title>
            <path d="M18.244 2.25h3.308l-7.227 8.26 8.502 11.24H16.17l-5.214-6.817L4.99 21.75H1.68l7.73-8.835L1.254 2.25H8.08l4.713 6.231zm-1.161 17.52h1.833L7.084 4.126H5.117z" />
          </svg>
        </a>
      </div>
    </header>
  );
}
