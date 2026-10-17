import { invoke } from "@tauri-apps/api/core";

/** A skills folder an agent reads (e.g. ~/.claude/skills). */
export interface SkillTarget {
  id: string;
  label: string;
  dir: string;
  /** The agent's own folder exists on this machine. */
  detected: boolean;
}

export interface SkillOrigin {
  repo: string;
  path: string;
  rev: string;
  /** Git tree hash of the skill's folder (empty for older installs). */
  hash: string;
}

export interface InstalledSkill {
  name: string;
  description: string;
  copies: { target: string; dir: string; link: boolean; origin: SkillOrigin | null }[];
}

export interface RepoSkill {
  name: string;
  description: string;
  path: string;
  files: number;
  hash: string;
}

export interface RepoListing {
  repo: string;
  rev: string;
  skills: RepoSkill[];
}

/** Repos offered without typing them in. */
export const FEATURED_SOURCES: { repo: string; label: string; description: string }[] = [
  { repo: "JuliusBrussee/caveman", label: "Caveman", description: "Terse output modes (caveman, ultracave, megacave) and lean coding workflows" },
  { repo: "anthropics/skills", label: "Anthropic", description: "Anthropic's skills: documents (pdf, docx, xlsx, pptx), frontend design, MCP builder, skill creator" },
];

const SOURCES_KEY = "winfer.skillSources";
const TARGETS_KEY = "winfer.skillTargets";

const readList = (key: string): string[] | null => {
  try {
    const value = JSON.parse(localStorage.getItem(key) ?? "null");
    return Array.isArray(value) && value.every((v) => typeof v === "string") ? value : null;
  } catch {
    return null;
  }
};

/** Repos the user added. */
export const customSources = (): string[] => readList(SOURCES_KEY) ?? [];
export const saveCustomSources = (repos: string[]) => localStorage.setItem(SOURCES_KEY, JSON.stringify(repos));

/** The agents skills are installed for, last picked (default: Claude Code and the shared folder). */
export const savedTargets = (): string[] => readList(TARGETS_KEY) ?? ["claude", "agents"];
export const saveTargets = (ids: string[]) => localStorage.setItem(TARGETS_KEY, JSON.stringify(ids));

/** `owner/repo` from what the user typed (a URL works too), or null. */
export function normalizeRepo(input: string): string | null {
  const m = input
    .trim()
    .replace(/^(https?:\/\/)?(www\.)?github\.com\//i, "")
    .match(/^([\w.-]+)\/([\w.-]+?)(\.git)?(\/.*)?$/);
  return m ? `${m[1]}/${m[2]}` : null;
}

export const fetchTargets = () => invoke<SkillTarget[]>("skills_targets");
export const fetchInstalledSkills = () => invoke<InstalledSkill[]>("skills_list");
export const fetchRepoSkills = (repo: string, fresh = false) => invoke<RepoListing>("skills_repo", { repo, fresh });
export const installSkill = (repo: string, path: string, targets: string[]) => invoke<string>("skills_install", { repo, path, targets });
export const uninstallSkill = (name: string, targets: string[]) => invoke<void>("skills_uninstall", { name, targets });
export const copySkill = (name: string, from: string, targets: string[]) => invoke<void>("skills_copy", { name, from, targets });

/** The origin of the first copy installed from a repo. */
export const skillOrigin = (skill: InstalledSkill) => skill.copies.find((c) => c.origin)?.origin ?? null;

/** True when a repo's listing has a changed version of an installed skill. */
export function isOutdated(skill: InstalledSkill, listing: RepoListing | undefined): boolean {
  const origin = skillOrigin(skill);
  if (!origin || !listing || listing.repo.toLowerCase() !== origin.repo.toLowerCase()) return false;
  const latest = listing.skills.find((s) => s.path === origin.path);
  if (!latest) return false;
  return origin.hash && latest.hash ? origin.hash !== latest.hash : listing.rev !== origin.rev;
}
