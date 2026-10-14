/**
 * The ChatGPT account winfer is signed in to (Sign in with ChatGPT, see src-tauri/src/chatgpt.rs).
 * Voice commands and the LLM use the user's ChatGPT plan through it.
 */
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

interface ChatGptState {
  signedIn: boolean;
  email: string | null;
  /** Models the account can use, once loaded. */
  models: string[];
  /** The plan can't call the Decisions API: voice commands are planned by the LLM instead. */
  decisionsUnavailable: boolean;
}

/** Accounts whose plan refused the Decisions API, so later sessions skip straight to the LLM. */
const REFUSED_KEY = "winfer.decisionsRefused";
const refused = (): string[] => {
  try {
    return JSON.parse(localStorage.getItem(REFUSED_KEY) ?? "[]");
  } catch {
    return [];
  }
};

export const useChatGpt = create<ChatGptState>(() => ({ signedIn: false, email: null, models: [], decisionsUnavailable: false }));

/** Records that this account's plan can't use the Decisions API. */
export function markDecisionsUnavailable() {
  const { email } = useChatGpt.getState();
  useChatGpt.setState({ decisionsUnavailable: true });
  if (email && !refused().includes(email)) localStorage.setItem(REFUSED_KEY, JSON.stringify([...refused(), email]));
}

export async function refreshChatGpt() {
  try {
    const status = await invoke<{ signedIn: boolean; email: string | null }>("chatgpt_status");
    useChatGpt.setState({ ...status, decisionsUnavailable: !!status.email && refused().includes(status.email) });
    if (status.signedIn && useChatGpt.getState().models.length === 0) void loadModels();
  } catch (err) {
    console.error("chatgpt_status failed", err);
  }
}

/** Opens the browser sign-in and waits for it to finish. */
export async function signInWithChatGpt() {
  const status = await invoke<{ signedIn: boolean; email: string | null }>("chatgpt_sign_in");
  useChatGpt.setState({ ...status, decisionsUnavailable: !!status.email && refused().includes(status.email) });
  void loadModels();
}

/** Stops a sign-in that's waiting for the browser (e.g. after an error page there). */
export const cancelChatGptSignIn = () => invoke("chatgpt_cancel_sign_in");

export async function signOutOfChatGpt() {
  await invoke("chatgpt_sign_out");
  useChatGpt.setState({ signedIn: false, email: null, models: [] });
}

async function loadModels() {
  try {
    useChatGpt.setState({ models: await invoke<string[]>("chatgpt_models") });
  } catch (err) {
    console.warn("couldn't list ChatGPT models", err);
  }
}
