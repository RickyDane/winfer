/**
 * The ChatGPT account winfer is signed in to (Sign in with ChatGPT, see src-tauri/src/chatgpt.rs).
 * The LLM (voice planner, prompt writer, …) uses the user's ChatGPT plan through it.
 */
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";

interface ChatGptState {
  signedIn: boolean;
  email: string | null;
  /** Models the account can use, once loaded. */
  models: string[];
}

export const useChatGpt = create<ChatGptState>(() => ({ signedIn: false, email: null, models: [] }));

export async function refreshChatGpt() {
  try {
    const status = await invoke<{ signedIn: boolean; email: string | null }>("chatgpt_status");
    useChatGpt.setState(status);
    if (status.signedIn && useChatGpt.getState().models.length === 0) void loadModels();
  } catch (err) {
    console.error("chatgpt_status failed", err);
  }
}

/** Opens the browser sign-in and waits for it to finish. */
export async function signInWithChatGpt() {
  const status = await invoke<{ signedIn: boolean; email: string | null }>("chatgpt_sign_in");
  useChatGpt.setState(status);
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
