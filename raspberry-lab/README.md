# raspberry-lab — 3-RPi isolated homelab

Abstract demonstration of provisioning a small, fully isolated Raspberry Pi
lab from scratch with Ansible: no internet, no cloud, no off-the-shelf
service. The point is the *shape* of a self-contained system — bootstrap,
control plane, storage, and a local inference worker — wired together on a
single flat LAN.

Status: **design + ansible stack complete; untested against hardware**
(this is an abstract lab, no physical Pis in scope).

## Nodes

| node | FQDN | IP | role |
|------|------|----|------|
| rpi-01 | `rpi-01.raspberry-lab` | 10.10.0.10 | head — ansible controller, dnsmasq (DNS+DHCP), chrony reference clock, sshd |
| rpi-02 | `rpi-02.raspberry-lab` | 10.10.0.11 | storage — MinIO (S3 API :9000, console :9001), bucket `lab` |
| rpi-03 | `rpi-03.raspberry-lab` | 10.10.0.12 | inference — llama.cpp `llama-server` :8080, model `Qwen2.5-1.5B-Instruct-Q4_K_M.gguf` |
| staging | `staging.raspberry-lab` | 10.10.0.2 | transient operator laptop, bootstrap only — plain HTTP file server on :8000 rooted at /srv |

Hardware assumption: Raspberry Pi 4 (4 GB) × 3, Raspberry Pi OS
(bookworm, 64-bit). Each Pi is flashed with two public keys written by
the Raspberry Pi Imager: the operator's (interactive) and the head's
(ansible control). The head's private key is scp'ed to rpi-01 as a
one-time manual step before the first playbook run.

## Network topology

```
            10.10.0.0/24 — isolated, no uplink, no gateway
            (wired Ethernet, one shared switch)

        ┌──────────────┐
        │  staging     │  10.10.0.2  (operator laptop, removed after
        │  (transient) │                bootstrap: /srv/files, /srv/models)
        └──────┬───────┘
               │
               │
   ┌───────────┼───────────────────────────────────┐
   │           │                                     │
┌──┴─────┐ ┌──┴─────┐  ┌─────────────────┐        │
│ rpi-01 │ │ rpi-02 │  │ rpi-03          │        │
│ .10    │ │ .11    │  │ .12             │        │
│  head  │ │storage │  │  inference      │        │
│        │ │        │  │                 │        │
│ansible │ │MinIO   │  │ llama-server    │        │
│dnsmasq │ │ :9000  │  │  :8080          │        │
│chrony  │ │ :9001  │  │ /srv/models     │        │
│ refclk │ │ bucket │  │                 │        │
└────────┘ │ `lab`  │  └─────────────────┘        │
           └────────┘                             │
   └─────────────────────────────────────────────┘
```

Flows:

- **bootstrap (staging → all)**: staging is the only node with
  off-lab material — model GGUF in `/srv/models`, llama.cpp tarball and
  minio/mc binaries in `/srv/files` — served over a plain HTTP file
  server on :8000 (the apt fallback, if `lab_apt_preseeded: false`, reads
  a debmirror tree from `/srv/apt` on the same server). It is unplugged
  once provisioned.
- **control (rpi-01 → rpi-02, rpi-03)**: SSH from the head runs both
  playbooks; dnsmasq serves `*.raspberry-lab` so names work after the
  hosts-file bootstrap window.
- **data (any → rpi-02)**: S3 API on 9000; bucket `lab` holds model
  artifacts and lab files.
- **inference (rpi-01/laptop → rpi-03:8080)**: OpenAI-compatible
  `llama-server` endpoint; the lab's "what local inference can do" hook.

## Design decisions

- **Flat /24, static netplan, no gateway, no RA.** An isolated lab needs
  zero network services beyond what the three Pis provide. Static
  netplan over DHCP is chosen because there is no DHCP server until
  dnsmasq is configured on rpi-01, and bootstrapping over DHCP is an
  extra moving part.
- **No internet, ever → cold-start via staging.** apt/pip/any download
  from outside the LAN is impossible by design, so the flashed image is
  *preseeded* (pi-gen: `ansible-core`, `dnsmasq`, `chrony`, `minio`,
  `git`, `curl`, `wget`); the staging laptop transports only the model
  file and the llama.cpp source tarball. The Ansible stack has a
  `lab_apt_preseeded: false` fallback that installs from a local mirror
  on staging, but the default path makes zero apt calls.
- **chrony: rpi-01 is the reference clock** (`local stratum 10`, no
  upstream). The other two sync to it. Without internet there is no
  stratum-1 source; a local reference is the honest answer.
- **MinIO over Ceph/NFS** for storage: single-node S3 is enough for a
  3-node lab, is one binary, and gives the inference node a sane way to
  fetch/serve model artifacts. Ceph's minimum sane topology (3 monitor +
  OSD disks) is more machinery than the demo justifies.
- **llama.cpp `llama-server` on rpi-03** with a 1.5B-class GGUF: this is
  the inference half of the lab and the tie to this repo's theme. 1.5B
  Q4 runs comfortably in 4 GB with headroom for context.
- **dnsmasq on the head** for DNS+DHCP after bootstrap: names
  (`rpi-02.raspberry-lab`) work for anything that stays on the lab
  network; /etc/hosts covers the window before dnsmasq is up.
- **Ansible as the only provisioning path.** No cloud-init, no
  pi-gen hooks beyond preseeded packages: one controller, two
  playbooks, idempotent re-runs.

## Ansible stack

```
raspberry-lab/ansible/
├── ansible.cfg
├── inventory.yml
├── group_vars/{common,head,storage,inference}.yml
├── playbooks/
│   ├── bootstrap.yml      # all nodes → base (hostname, hosts, netplan, chrony)
│   └── deploy-lab.yml     # head→orchestrator, storage→storage, inference→inference
└── roles/
    ├── base/              # + netplan.yaml.j2, chrony-head.conf.j2, chrony-client.conf.j2
    ├── orchestrator/      # + dnsmasq.conf.j2, lab-check.sh.j2
    ├── storage/           # + templates/minio.service.j2
    └── inference/         # + templates/llama-server.service.j2
```

Sequence:

1. On the laptop: generate two ed25519 keypairs (operator + head/lab),
   flash the three Pis (RPi OS Lite, bookworm 64-bit) with both public
   keys written by the Imager, `scp` the head's private key to rpi-01.
   Start a plain HTTP file server on 10.10.0.2 rooted at /srv.
2. From the head (rpi-01), in `raspberry-lab/ansible/`:
   `ansible-playbook playbooks/bootstrap.yml --check`, then without
   `--check`.
3. `ansible-playbook playbooks/deploy-lab.yml`
4. Unplug staging.

Notes:

- Idempotency is the contract: templates + `creates:` guards everywhere;
  the unavoidable `command` tasks (`mc` alias/bucket, `make`, `curl
  /health`) are marker-guarded or documented as safe no-op re-runs.
- `minio_root_pass` is a placeholder — rotate it after first console
  login; it lives in `group_vars/storage.yml`, not in a playbook.

## Verification (once hardware exists)

```sh
# from rpi-01, in raspberry-lab/ansible/
ansible -i inventory.yml all -m ping
curl -fsS http://10.10.0.11:9000/minio/health/live
curl -fsS http://10.10.0.12:8080/health
curl -fsS http://10.10.0.12:8080/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"Qwen2.5-1.5B-Instruct-Q4_K_M.gguf","messages":[{"role":"user","content":"ping"}]}'
~/raspberry-lab/lab-check.sh     # orchestrator's smoke script: ping+minio+llama
```

## Outcome

Design and the full Ansible stack are complete and internally consistent
(inventory → group vars → roles → playbooks all resolve against the same
frozen names). No hardware was exercised: the lab is an abstract
demonstration, and every claim above is a design decision, not a
verified run.
