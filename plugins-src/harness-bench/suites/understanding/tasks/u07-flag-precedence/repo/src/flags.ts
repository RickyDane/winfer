export interface Flags {
  newCheckout: boolean;
  searchV2: boolean;
  maxUploadMB: number;
  betaBanner: boolean;
  theme: string;
}

declare const __FLAGS__: Flags;

const raw: Flags = __FLAGS__;

/** The flags the app actually uses. */
export const flags: Flags = {
  ...raw,
  // The CDN rejects uploads above 100 MB anyway.
  maxUploadMB: Math.min(raw.maxUploadMB, 100),
  // The beta banner invites users to try the new checkout; hide it once that's on for everyone.
  betaBanner: raw.betaBanner && !raw.newCheckout,
};
