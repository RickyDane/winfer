import { create } from "zustand";

/** The race or A/B test whose results modal is open. */
export const useRaceResults = create<{ raceId: string | null }>(() => ({ raceId: null }));

export const openRaceResults = (raceId: string) => useRaceResults.setState({ raceId });
export const closeRaceResults = () => useRaceResults.setState({ raceId: null });
