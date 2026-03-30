# meshtastic-bot-discord

A simple Discord bot for managing a [Meshtastic](https://meshtastic.org) node and
for communicating with other nodes on the Meshtastic network.

Build and execution instructions tbd.

## Setup

You will need [cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html)
if you wish to install this from scratch. Copy `cargo.toml.default` to `cargo.toml`
in the root of the repo and update it accordingly. You'll need to set up your own
Discord application and bot user, which you can do at their
[developer portal](https://discord.com/developers/applications).

## Execution

You can run the development version with `cargo run` and you can build the
production release with `cargo build --release`. Send SIGINT or SIGTERM to end
execution.

## Bugs

Yes, they exist. No, you may not report them. (yet).
Come to the Hackspace in person for suppport :)

## License

Copyright 2026 Cambridge Hackspace, Inc. All rights reserved.

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at
    
  [https://apache.org/licenses/LICENSE-2.0](https://apache.org/licenses/LICENSE-2.0)

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.

Meshtastic is not a trademark of Cambridge Hackspace or any of its affiliates.
