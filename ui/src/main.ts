import { mount } from "svelte";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./app.css";
import App from "./App.svelte";
import MiniPlayer from "./lib/MiniPlayer.svelte";

// Both windows load this page: the mini player's window gets the mini player.
const target = document.getElementById("app")!;
export default getCurrentWindow().label === "mini" ? mount(MiniPlayer, { target }) : mount(App, { target });
