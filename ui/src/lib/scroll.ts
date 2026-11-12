/** Whether a scroll box is at its end, within `slack` px. */
export function atBottom(scrollTop: number, clientHeight: number, scrollHeight: number, slack = 20): boolean {
  return scrollTop + clientHeight >= scrollHeight - slack;
}
