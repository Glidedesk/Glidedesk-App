// Small consistent icon set (24px grid, 1.75 stroke), inline so there are no
// extra requests and they follow the text colour.
import type { SVGProps } from "react";

type P = SVGProps<SVGSVGElement> & { size?: number };

function Svg({ size = 18, children, ...rest }: P) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...rest}
    >
      {children}
    </svg>
  );
}

export const Icon = {
  Layout: (p: P) => (
    <Svg {...p}>
      <rect x="2.5" y="5" width="8" height="6" rx="1.5" />
      <rect x="13.5" y="5" width="8" height="6" rx="1.5" />
      <rect x="8" y="14" width="8" height="6" rx="1.5" />
    </Svg>
  ),
  Computers: (p: P) => (
    <Svg {...p}>
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8M12 16v4" />
    </Svg>
  ),
  Clipboard: (p: P) => (
    <Svg {...p}>
      <rect x="6" y="4" width="12" height="17" rx="2" />
      <path d="M9 4.5V3h6v1.5M9 10h6M9 14h4" />
    </Svg>
  ),
  Keyboard: (p: P) => (
    <Svg {...p}>
      <rect x="2.5" y="6" width="19" height="12" rx="2" />
      <path d="M6 10h.01M9.5 10h.01M13 10h.01M16.5 10h.01M7 14h10" />
    </Svg>
  ),
  Network: (p: P) => (
    <Svg {...p}>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
    </Svg>
  ),
  Settings: (p: P) => (
    <Svg {...p}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1Z" />
    </Svg>
  ),
  Wrench: (p: P) => (
    <Svg {...p}>
      <path d="M14.7 6.3a4 4 0 0 0 5 5L22 13.6 13.6 22l-2.3-2.3a4 4 0 0 0-5-5L3.9 12.4a4 4 0 0 1 5.6-5.6L11 8.3l3.7-3.7Z" />
    </Svg>
  ),
  Home: (p: P) => (
    <Svg {...p}>
      <path d="m3 11 9-7 9 7" />
      <path d="M5 10v10h14V10" />
    </Svg>
  ),
  Play: (p: P) => (
    <Svg {...p}>
      <path d="M7 5v14l11-7Z" />
    </Svg>
  ),
  Stop: (p: P) => (
    <Svg {...p}>
      <rect x="6" y="6" width="12" height="12" rx="2" />
    </Svg>
  ),
  Refresh: (p: P) => (
    <Svg {...p}>
      <path d="M20 11a8 8 0 0 0-14.3-4.9L4 8M4 4v4h4M4 13a8 8 0 0 0 14.3 4.9L20 16M20 20v-4h-4" />
    </Svg>
  ),
  Eye: (p: P) => (
    <Svg {...p}>
      <path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12Z" />
      <circle cx="12" cy="12" r="3" />
    </Svg>
  ),
  Check: (p: P) => (
    <Svg {...p}>
      <path d="m5 12.5 4.5 4.5L19 7.5" />
    </Svg>
  ),
  Info: (p: P) => (
    <Svg {...p}>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5M12 8h.01" />
    </Svg>
  ),
  Warning: (p: P) => (
    <Svg {...p}>
      <path d="M10.3 3.9 2.4 17.5A2 2 0 0 0 4.1 20.5h15.8a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" />
      <path d="M12 9v4M12 17h.01" />
    </Svg>
  ),
  Error: (p: P) => (
    <Svg {...p}>
      <circle cx="12" cy="12" r="9" />
      <path d="m9 9 6 6M15 9l-6 6" />
    </Svg>
  ),
  Menu: (p: P) => (
    <Svg {...p}>
      <path d="M4 7h16M4 12h16M4 17h16" />
    </Svg>
  ),
  Close: (p: P) => (
    <Svg {...p}>
      <path d="M6 6l12 12M18 6 6 18" />
    </Svg>
  ),
  Power: (p: P) => (
    <Svg {...p}>
      <path d="M12 3v8M6.3 6.3a8 8 0 1 0 11.4 0" />
    </Svg>
  ),
  Lock: (p: P) => (
    <Svg {...p}>
      <rect x="5" y="11" width="14" height="9" rx="2" />
      <path d="M8 11V8a4 4 0 0 1 8 0v3" />
    </Svg>
  ),
  Apple: (p: P) => (
    <Svg {...p}>
      <path d="M16.4 12.6c0-2.3 1.9-3.4 2-3.5a4.3 4.3 0 0 0-3.4-1.8c-1.4-.1-2.8.9-3.5.9s-1.9-.9-3.1-.8a4.6 4.6 0 0 0-3.9 2.4c-1.7 2.9-.4 7.2 1.2 9.5.8 1.1 1.7 2.4 2.9 2.4s1.6-.8 3-.8 1.8.8 3 .7c1.3 0 2.1-1.2 2.9-2.3a9.8 9.8 0 0 0 1.3-2.7 4 4 0 0 1-2.4-3.8ZM14.1 5.8a4.1 4.1 0 0 0 1-3 4.2 4.2 0 0 0-2.7 1.4 3.9 3.9 0 0 0-1 2.9 3.5 3.5 0 0 0 2.7-1.3Z" />
    </Svg>
  ),
  Windows: (p: P) => (
    <Svg {...p}>
      <path d="M3 5.5 10.5 4.5v7H3ZM12 4.3 21 3v8.5h-9ZM3 12.5h7.5v7L3 18.5ZM12 12.5h9V21l-9-1.3Z" />
    </Svg>
  ),
  Linux: (p: P) => (
    <Svg {...p}>
      <path d="M12 3c-2 0-3 1.8-3 4 0 1.4.3 2.3-.7 3.9C7 12.9 5.5 15 6 17.5c.4 2 2.5 3.5 6 3.5s5.6-1.5 6-3.5c.5-2.5-1-4.6-2.3-6.6-1-1.6-.7-2.5-.7-3.9 0-2.2-1-4-3-4Z" />
      <path d="M10.5 8h.01M13.5 8h.01M11 10.5h2" />
    </Svg>
  ),
  Logo: (p: P) => (
    <Svg {...p} strokeWidth={2}>
      <rect x="2.5" y="6" width="7.5" height="6.5" rx="1.5" />
      <rect x="14" y="6" width="7.5" height="6.5" rx="1.5" />
      <path d="M6 16.5c2.5 3 9.5 3 12 0" />
    </Svg>
  ),
};

export function PlatformIcon({ platform, size = 16 }: { platform: string | null | undefined; size?: number }) {
  if (platform === "macos") return <Icon.Apple size={size} />;
  if (platform === "windows") return <Icon.Windows size={size} />;
  if (platform === "linux") return <Icon.Linux size={size} />;
  return <Icon.Computers size={size} />;
}
