# <img src="brand/parlando-wordmark.svg" width="260" alt="Parlando">

Parlando runs two-player dialogue-game experiments with humans and software agents. A game package contains WebAssembly mechanics and a browser application. One shared host can serve several games, and each experiment records the package revision and settings used for its sessions.

```sh
cargo install --locked parlando --bin parlando
```

Start with the [experimenter manual](https://coli-saar.github.io/parlando/manual/). Its [first walkthrough](https://coli-saar.github.io/parlando/manual/start/getting-started/) takes you from installation to playing both seats of a local game. Follow [package a game](https://coli-saar.github.io/parlando/manual/build/packages/) and [deploy to a shared host](https://coli-saar.github.io/parlando/manual/deploy/) when you are ready to run a study.

For maintainers, see the [release procedure](docs/releasing-parlando.md) and the [internals atlas](developer/code-atlas/index.html).

## Preview the manual

```sh
hugo server --source docs/web
```

Open [the local site](http://127.0.0.1:1313/parlando/).
