<h1 align="center">Yurlungr</h1>

<p align="center">
  <b>Coding agents in your pocket.</b><br>
  Chat with Claude Code, Codex, Gemini CLI and Antigravity running natively on your Android phone,<br>
  in a built-in Linux runtime. Build, git and ship from the same app. One APK, no Termux install.
</p>

This repository hosts Yurlungr's **releases** (APKs, checksums and release notes). The source code is not
public.

## Download

Grab the APK for your phone's CPU from the [latest release](https://github.com/mirimomekiku/yurlungr/releases/latest)
and install it (allow installs from your browser or file manager when asked):

| APK | For |
|---|---|
| `yurlungr-<version>-arm64-v8a.apk` | Almost every phone from the last several years (recommended) |
| `yurlungr-<version>-armeabi-v7a.apk` | Older 32-bit phones |
| `yurlungr-<version>-x86_64.apk` / `-x86.apk` | Emulators and x86 devices |
| `yurlungr-<version>-universal.apk` | Any of the above, if you're unsure (slightly larger) |

Not sure? `adb shell getprop ro.product.cpu.abi` prints your phone's ABI. Each release lists SHA-256
checksums in `SHA256SUMS.txt`.

**Requirements:** Android 7.0+ (arm64 is the tested target) and about 4 GB free.

## Highlights

- **Real agents, really on-device.** Each chat starts the agent's own CLI and talks to it over the
  [Agent Client Protocol](https://agentclientprotocol.com): streaming replies, plans, tool calls with diffs,
  permission prompts, slash commands, model and mode pickers, image and file attachments.
- **An embedded Linux.** Node, git, Python, OpenJDK, Gradle and the Android SDK run inside the app, native on
  arm64, with no proot and no separate Termux app.
- **Build APKs on the phone.** Agents or you run `./gradlew assembleDebug`, and `yurlungr-install` opens the
  system installer.
- **Projects & git.** File tree, editor, changes with diffs, commit, pull and push, branches and a coloured
  commit graph.
- **Stays alive.** A foreground service keeps agents running with the screen off, and you get a heads-up
  notification when an agent needs your answer.

## Getting started

1. **Set up the Linux environment.** The first-run System checks screen offers *Install*. Fix anything
   amber: battery optimisation, notifications, and on Android 12+ the background process limit.
2. **Install the tools and agents.** System checks → **Install tools** installs git, Node.js, Python, the
   coding agents and any optional packs in one run.
3. **Sign in.** Each agent's card has *Sign in*, which opens its own login.
4. **Chat.** Pick an agent and a project folder on the start screen and type.

## License

Yurlungr is made available under the [PolyForm Noncommercial License 1.0.0](LICENSE).

Required Notice: Copyright (c) 2026 dlord213

For permissions outside those terms, contact the licensor through
[GitHub](https://github.com/mirimomekiku) to arrange a separate license.

Yurlungr is an independent project, not affiliated with Anthropic, OpenAI, Google or other agent makers.
Names and logos belong to their owners.
