// Site nav data — single source of truth for sidebars in DocLayout
// and for the home page's "what to read first" picker.

export type NavItem = { href: string; label: string; summary?: string };
export type NavSection = { title: string; items: NavItem[] };

export const docsNav: NavSection[] = [
  {
    title: "Get going",
    items: [
      { href: "/start", label: "Your first contract", summary: "Tutorial" },
      { href: "/install", label: "Install", summary: "One command" },
      { href: "/concepts", label: "Concepts", summary: "What is Verity?" },
    ],
  },
  {
    title: "Guides",
    items: [
      { href: "/guides", label: "Index" },
      { href: "/guides/starter", label: "Reading the starter" },
      { href: "/guides/proofs", label: "Writing proof obligations" },
      { href: "/guides/audits", label: "Reading audit output" },
      { href: "/guides/foundry", label: "Foundry interop" },
    ],
  },
  {
    title: "Reference",
    items: [
      { href: "/reference/cli", label: "CLI commands" },
      { href: "/reference/config", label: "tama.toml & lockfile" },
      { href: "/reference/manifest", label: "Contract manifest" },
      { href: "/reference/artifacts", label: "Generated artifacts" },
    ],
  },
  {
    title: "Operate",
    items: [
      { href: "/troubleshooting", label: "Troubleshooting" },
      { href: "/limitations", label: "Limitations" },
      { href: "/privacy", label: "Privacy" },
    ],
  },
];

export const flatDocs = docsNav.flatMap((s) => s.items);
