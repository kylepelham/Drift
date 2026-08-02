import { render } from "solid-js/web"
import { App } from "./app"
import { runtimeNameFrom } from "./runtime"
import "./styles/app.css"

const root = document.getElementById("root")!
document.documentElement.dataset.runtime = runtimeNameFrom(window.location)
root.replaceChildren()
render(() => <App />, root)
