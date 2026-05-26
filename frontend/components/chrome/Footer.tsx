import Image from "next/image";

export function Footer() {
  return (
    <footer className="sticky bottom-0 z-40 border-border border-t bg-background">
      <div className="mx-auto flex h-12 max-w-6xl items-center justify-center px-6 text-muted-foreground text-sm">
        Courtesy of
        <a
          href="https://x.com/_DASMAC_"
          target="_blank"
          rel="noopener noreferrer"
          className="ml-2 inline-flex items-center hover:opacity-80"
        >
          <Image
            src="/dasmac-wordmark.png"
            alt="DASMAC"
            width={80}
            height={20}
            suppressHydrationWarning
          />
        </a>
      </div>
    </footer>
  );
}
