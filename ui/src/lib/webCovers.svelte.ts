// Whether covers from the internet are on (the setting itself is kept by the
// app, in output.json). Cover URLs carry `web` while they are, so switching
// them loads every cover again, and a picture the webview kept never outlives
// the setting.
export const webCovers = $state({ on: false });
