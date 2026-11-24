---
format: aep.planning-md/3
id: story:the-node-has-the-jvm-tools-of-the-workstation
kind: story
status: active
title: The build node has the JVM tools a gate step finds on the workstation
relations:
- serves: vision:O6
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-24T15:37:54Z", actor: "agent:slot", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-24T15:37:54Z", actor: "agent:slot", revision: 3}
---
# The build node has the JVM tools a gate step finds on the workstation

## Why

On 2026-11-24 a session reported JVM gate steps failing on the build node: no `/etc/java21-openjdk`, `javac` crashing, no Groovy. The toolchain parity check covers rust, go, node and C compilers only, so these jobs start on the node and fail there.

## Acceptance

- The parity manifest (`toolchains.sh`) lists the JDKs, `javac`, Groovy and Maven a job resolves, with digests, on the workstation and on the node.
- A job whose tree needs one of them (a `pom.xml`, `build.gradle`, `*.groovy`, `.sdkmanrc`) waits for parity like a rust job, and gives up after the same bound.
- The node image or its setup script installs the same JDK packages and `/etc/java*` configuration as the workstation.
- Red first: a manifest pair that differs in `javac` holds a job with a `pom.xml`.
