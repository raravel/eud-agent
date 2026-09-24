/**
 * The leading eight characters of a session id: the handle the session lists
 * show beside each session and team Map prompts quote for the parent EPS
 * session, so a person can name one session without the full UUID. Mirrors
 * `session::short_session_id` in Rust.
 */
export function shortSessionId(id: string): string {
  return id.slice(0, 8);
}
