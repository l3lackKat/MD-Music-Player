import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./app.css";
import App from "./App.svelte";
import MiniPlayer from "./lib/MiniPlayer.svelte";
import { api } from "./lib/api";
import { startTheme } from "./lib/theme";
import { webCovers } from "./lib/webCovers.svelte";

startTheme();

// Both windows load this page: the mini player's window gets the mini player.
// The cover setting is read first, so the first covers asked for carry it.
const target = document.getElementById("app")!;
api
  .getWebCovers()
  .then((on) => (webCovers.on = on))
  .catch(() => {})
  .finally(() => (getCurrentWindow().label === "mini" ? mount(MiniPlayer, { target }) : mount(App, { target })));
