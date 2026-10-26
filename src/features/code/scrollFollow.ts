/** A scroll event is not necessarily user input: anchoring, offscreen row
 * measurements, and smooth scrolling also emit it. Only a deliberate downward
 * return to the actual bottom may resume following after the reader paused it. */
export function followAfterScroll(
  following: boolean,
  previousTop: number,
  top: number,
  distance: number,
  intent: number,
): boolean {
  if (top < previousTop && distance > 2) return false;
  if (!following)
    return intent >= 0 && top > previousTop && distance <= 2;
  return true;
}
