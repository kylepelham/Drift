import { render } from "solid-js/web"
import { App } from "./app"
import "./styles/app.css"

const root = document.getElementById("root")!
root.replaceChildren()
render(() => <App />, root)
