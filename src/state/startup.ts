import { persisted } from "./persist"

export const splashMascotAnimations = ["bounce", "float", "pulse", "still"] as const
export type SplashMascotAnimation = (typeof splashMascotAnimations)[number]
export const splashExitAnimations = ["wave", "fade", "lift"] as const
export type SplashExitAnimation = (typeof splashExitAnimations)[number]
export const splashDurations = [1500, 3200, 5000] as const

export const [splashEnabled, setSplashEnabled] = persisted("drift.splash.enabled", true)
export const [splashMascotAnimation, setSplashMascotAnimation] = persisted<SplashMascotAnimation>(
  "drift.splash.mascot",
  "bounce",
)
export const [splashExitAnimation, setSplashExitAnimation] = persisted<SplashExitAnimation>(
  "drift.splash.exit",
  "wave",
)
export const [splashDuration, setSplashDuration] = persisted<number>("drift.splash.duration", 3200)
export const [splashFont, setSplashFont] = persisted("drift.splash.font", "")
