# M09-005 — Native/offline Controller specialization backend investigation

**Investigation status:** Complete; backend selection deferred
**As of:** 2026-09-07
**Repository HEAD inspected:** `744ff7e`

## Executive result

No training backend is selected by this investigation.

The smallest justified future path is a conditional, offline LoRA experiment in a separate training/export environment using an explicitly selected original Qwen3 Hugging Face checkpoint, with `ms-swift` as the first framework to qualify. The documented Qwen recipe uses `Qwen/Qwen3-8B`, the post-trained/instruct checkpoint; `Qwen/Qwen3-8B-Base` is the separate pretraining checkpoint and is not interchangeable with it. The recipe explicitly supports Qwen3-8B LoRA, local model/dataset paths, adapter merge, and reports a 22 GB GPU requirement for the example. This is a training/export toolchain only; it must not become an Orc runtime dependency.

The current Orc runtime should continue to consume one candidate GGUF through the existing `LocalInferenceRuntime` boundary. The candidate should be a merged Hugging Face checkpoint converted and quantized to GGUF after training. A standalone adapter artifact is not the minimum compatible Orc artifact because the current `LocalRuntimeConfig` has one `model_path` and `LlamaCppRuntime` has no adapter-loading configuration.

Actual execution is deferred. The local host has no usable GPU, only 14 GiB RAM, no Qwen3 model or GGUF file, no installed llama CLI/Hugging Face downloader, and no populated canonical experience table in the configured global registry. Dataset size, capability coverage, and real-Qwen baseline score therefore remain unknown. No capacity, score, readiness label, or promotion claim is inferred from fixtures or documentation.

## Repository evidence

| Surface | Verified repository fact | Consequence |
| --- | --- | --- |
| `Cargo.toml`, `Cargo.lock` | Orc pins optional `llama-cpp-2`/`llama-cpp-sys-2` at `0.1.155`; the feature is `llama-cpp`. | The production native boundary is an inference dependency, not a trainer selection. |
| `src/local_runtime.rs` | `LocalInferenceRuntime` is model-independent; `LocalRuntimeConfig` contains one model path, context length, and optional threads. | Candidate inference remains replaceable and model-file based. |
| `src/local_runtime/llama_cpp.rs` | `LlamaCppRuntime::from_config` loads one GGUF file; request-time structured decoding is private to the adapter. | No adapter path, training handle, or trainer type crosses the Controller boundary. |
| `docs/orc-next/DECISIONS.md` D-004/D-006/D-007 | Qwen3 8B + llama.cpp/GGUF is the initial inference direction; learning is periodic and curated; Orc runtime stays Rust/native and training/export is separate. | Training may be an external, isolated toolchain, but it must preserve native Orc runtime boundaries. |
| M09-001 | `OrcApp::controller_experience_snapshot()` returns schema 1, complete Active canonical examples, in deterministic ID order, without trainer-specific transformation. | This typed snapshot is the only future training-input authority. |
| M08-011 | `controller_experience_inventory()` counts the complete canonical global-registry table without readiness or quality inference. | Inventory must be measured before any transformation or split is designed. |
| M09-002/M09-003 | One fixed nine-capability semantic suite and a real-model report path exist; `ORC_QWEN3_GGUF` is the model-file convention. | Candidate evaluation must reuse this exact suite and production-aligned requests. |
| M09-004 | Candidate promotion requires comparable reports, strict global improvement, no execution/error increase, no capability or baseline-pass regressions, and no new failure classes. | Training success alone cannot promote or replace the default model. |

## Upstream evidence and versions

The following were checked against upstream sources on 2026-09-07. A moving `main`/`master` page is recorded as moving where upstream did not expose a stable commit in the inspected page; a future implementation must pin exact revisions before downloading or running anything.

| Source/revision | Verified capability or format | License / operational note |
| --- | --- | --- |
| [Qwen/Qwen3-8B at HF revision `b968826d9c46dd6066d109eabc6255188de91218`](https://huggingface.co/Qwen/Qwen3-8B/tree/b968826d9c46dd6066d109eabc6255188de91218) | Official post-trained/instruct checkpoint used by Qwen’s documented ms-swift example; it is Transformers/Safetensors, 16.4 GB in five shards, and `config.json` identifies `Qwen3ForCausalLM`, model type `qwen3`, BF16, 36 layers, 4096 hidden size, and vocabulary 151936. | Apache-2.0 model license. This is the conditional ms-swift training input, not an existing quantized GGUF. The separate [Qwen/Qwen3-8B-Base](https://huggingface.co/Qwen/Qwen3-8B-Base) checkpoint is the pretraining model; the two checkpoints are not interchangeable and must not be silently substituted. |
| [Qwen/Qwen3-8B-GGUF](https://huggingface.co/Qwen/Qwen3-8B-GGUF) | Official quantized inference artifacts include Q4_K_M (5.03 GB), Q5 variants, Q6_K, and Q8_0. Qwen documents llama.cpp use and identifies GGUF as carrying weights, model parameters, generation defaults, and tokenizer metadata. | Apache-2.0 model license. This is the current inference artifact family; the page does not establish that a Q4 GGUF is a supported LoRA-training input. |
| [Qwen Qwen3 llama.cpp guide](https://github.com/QwenLM/Qwen3/blob/main/docs/source/run_locally/llama.cpp.md), `main` | Qwen documents official Qwen3 GGUF use with llama.cpp, local GGUF execution, and HF-to-GGUF conversion. The conversion step requires Python and `transformers`. | Confirms inference/conversion compatibility, not native training support. |
| [Qwen Qwen3 ms-swift guide](https://github.com/QwenLM/Qwen3/blob/main/docs/source/training/ms_swift.md), `main` | Qwen’s Qwen3-8B example uses `swift sft --train_type lora`; it supports local `--model` and `--dataset` paths for offline operation and `swift export --merge_lora true`. The example reports 22 GB GPU memory for LoRA and 4 × 60 GB for the shown full-tuning setup. | Python/PyTorch training toolchain. It is suitable only outside the Orc runtime boundary. |
| [ms-swift v4.5.2, commit `ff51287`](https://github.com/modelscope/ms-swift/releases/tag/v4.5.2) | Current observed release; upstream project describes PEFT/full-parameter CPT/SFT/DPO/GRPO support and Qwen3-family support. | Apache-2.0 project license. Exact release and dependencies must be locked for an experiment. |
| [llama.cpp b10837, commit `5202104`](https://github.com/ggml-org/llama.cpp/releases/tag/b10837) | Current observed upstream release. `examples/training` contains `llama-finetune`; the official [training README](https://github.com/ggml-org/llama.cpp/blob/master/examples/training/README.md) says finetuning is technically functional for FP32 models but WIP, and reports Stories 260K/LLaMA 3.2 1B working with 24 GB. | MIT project license. The README gives no Qwen3-8B training result or stable production promise. |
| [llama.cpp LoRA conversion](https://github.com/ggml-org/llama.cpp/blob/master/convert_lora_to_gguf.py) and [LoRA conversion/inference test](https://github.com/ggml-org/llama.cpp/blob/master/tests/test-lora-conversion-inference.sh), `master` | Upstream can convert a Hugging Face PEFT LoRA adapter to GGUF and test conversion plus merged/exported inference. | The scripts are Python-based. `convert_lora_to_gguf.py` requires base-model configuration/metadata (such as `config.json` and tokenizer metadata), not the actual base weights, for adapter conversion. Actual base artifacts are required separately for merging/applying the adapter or validating inference; Qwen3-specific adapter validation is not established by these sources. |
| [llama-cpp-2 0.1.156 docs](https://docs.rs/crate/llama-cpp-2/latest) | 0.1.156 was current upstream on 2026-09-02; Orc intentionally remains pinned to 0.1.155. | The binding exposes the inference integration used by Orc; it is not a training API dependency in this repository. |
| [LlamaFactory v0.9.5 releases](https://github.com/hiyouga/LlamaFactory/releases) and [project README](https://github.com/hiyouga/LlamaFactory) | Supports Qwen3 and SFT/full/LoRA/QLoRA paths, with merge/export examples. | Python/PyTorch/bitsandbytes-oriented framework. Credible fallback, but Qwen’s own ms-swift recipe is the smaller first qualification target. |

## Format and compatibility constraints

Training, adapter production, conversion, quantization, and inference are separate boundaries:

1. The canonical input is the M09-001 typed Active snapshot, not a GGUF file and not runtime memory.
2. A future transformation may produce trainer-specific records in an external work directory. That transform must record the source snapshot schema/count and its own transform revision; it must not rewrite the canonical examples or become evaluation authority.
3. LoRA/QLoRA training requires one explicitly selected original Qwen3 checkpoint and matching tokenizer in a trainer-supported format. The conditional ms-swift path names `Qwen/Qwen3-8B`, the post-trained/instruct checkpoint, whose verified files are sharded Safetensors/BF16. `Qwen/Qwen3-8B-Base` is a separate pretraining checkpoint and is not interchangeable with the instruct checkpoint; selecting it would require separate qualification. The official GGUF page establishes local inference, not that a quantized GGUF is a valid input to the chosen trainer.
4. The minimum candidate lineage should be: exact selected Qwen3 checkpoint identity (including whether `Qwen/Qwen3-8B` or `Qwen/Qwen3-8B-Base`), repository revision, and file digests → tokenizer/config revision → snapshot schema/count and transform revision → trainer/framework/dependency lock → seed and hyperparameters → PEFT adapter → merged HF checkpoint → conversion revision and log → quantization type and parameters → candidate GGUF digest.
5. The current Orc runtime should receive the merged candidate GGUF as one `model_path`. Keeping a PEFT adapter separately is useful for lineage and possible future runtime work, but it is not the current production artifact boundary.
6. `convert_hf_to_gguf.py` and `convert_lora_to_gguf.py` are Python tools. This does not violate D-007 if they remain an explicitly separate offline training/export environment; adding them to Orc’s runtime or build is out of scope.

## Alternatives and trade-offs

### A. Native llama.cpp training

This is the strongest fit for D-007 in language/runtime terms. Upstream has a native C++ `llama-finetune` example and can run CPU training, but its own README labels the feature WIP, limits the statement to FP32 models and limited hardware, and reports a 1B/24 GB demonstration. No inspected upstream source establishes Qwen3-8B training quality, LoRA/QLoRA support for this model, or a reproducible 8B recipe. It also does not remove the separate Python conversion step needed to move HF/PEFT artifacts through the GGUF boundary.

**Disposition:** Do not select for the first Orc specialization experiment. Revisit only with a pinned upstream revision, a Qwen3-8B training proof, explicit memory measurements, and an artifact/evaluation proof.

### B. ms-swift LoRA, then merged HF → GGUF

This is the smallest verified Qwen-specific route. Qwen publishes the exact Qwen3-8B instruct-checkpoint LoRA command, local/offline model and dataset path convention, adapter merge command, and a 22 GB GPU example. The command names `Qwen/Qwen3-8B`; `Qwen/Qwen3-8B-Base` is a separate pretraining checkpoint and is not an interchangeable input. This route preserves the existing Orc inference boundary after export, avoids full-model mutation, and leaves the adapter available for lineage.

Its costs are a Python/PyTorch training environment, the selected original HF checkpoint and matching tokenizer, a separate conversion/quantization stage, and a still-unverified fit between the eventual Orc dataset transform and the trainer’s message format.

**Disposition:** Conditional first candidate, not selected for execution until readiness blockers clear.

### C. LlamaFactory LoRA/QLoRA or full tuning

LlamaFactory is a credible Qwen3-capable alternative with LoRA, QLoRA, full tuning, and merge/export workflows. It has a broader configuration surface and could be useful if later experiments need QLoRA or a different distributed backend.

It is also Python/PyTorch-centered, introduces more framework/configuration choices than the first experiment needs, and the inspected project documentation did not provide a Qwen3-8B memory figure as direct as Qwen’s ms-swift recipe.

**Disposition:** Keep as a fallback qualification task; do not add it to Orc now.

### D. Full fine-tuning

Qwen’s documented example uses 4 × 60 GB for the shown full-tuning setup. Full tuning produces the largest artifact and the largest regression/lineage risk while the repository has no measured dataset inventory or baseline score.

**Disposition:** Defer. It is not the smallest viable path for this repository.

## Local readiness evidence

All checks below were read-only and performed in the repository workspace.

### Hardware and tools

- Host: x86_64 Linux, Intel Core i5-14600K, 14 physical cores / 20 logical CPUs.
- RAM: 14 GiB total, 7.7 GiB available at inspection; 7.5 GiB swap.
- GPU: no usable NVIDIA device; `nvidia-smi` was present but reported that it could not communicate with the driver. No other accelerator evidence was available.
- Present: `cargo`, `cmake`, `clang++`, and `python3`.
- Not found on `PATH`: `huggingface-cli`, `ollama`, `llama-cli`, and `llama-server`.
- No `ORC_QWEN3_GGUF` environment variable was set.

This does not prove that CPU inference or all future training configurations are impossible. It does prove that the documented 22 GB GPU LoRA example cannot be run on this host as inspected, and that no local model artifact is available for the required baseline.

### Model and dataset

- No `*.gguf` file was found under the repository, `/home/reza/.cache`, or `/tmp`.
- No local Qwen/Hugging Face model directory was found in those inspected locations.
- The project DB `.orc/orc.db` records the global registry path as `/home/reza/.local/share/orc/agents.db`.
- A read-only SQLite inspection of that registry found the agent tables but no `controller_experience_examples` table. Therefore the actual canonical experience count is **not measurable from the current on-disk registry**; it must not be reported as zero. Opening the application may perform schema creation/migration, so that was not done during this non-mutating investigation.
- Repository tests and fixtures prove schema behavior only. They are not production dataset inventory evidence.

Readiness is consequently:

| Prerequisite | State | Evidence |
| --- | --- | --- |
| Canonical Active dataset | Unknown / registry not provisioned | No experience table in configured global registry; no count or capability inventory can be asserted. |
| Selected original Qwen3 checkpoint weights (documented path: `Qwen/Qwen3-8B`) | Unavailable locally | No HF cache/model directory found. |
| Qwen3 GGUF baseline artifact | Unavailable locally | No GGUF file and no `ORC_QWEN3_GGUF`. |
| Real baseline score | Not recorded | M09-003 explicitly reports no score without the model. |
| Training GPU | Unavailable | No usable accelerator; 14 GiB system RAM. |
| Offline trainer/export lock | Not prepared | No trainer or large toolchain was installed, as required by task non-goals. |

## Recommended future boundary and validation sequence

The following is a design boundary, not an implementation or backend selection:

```text
canonical Active snapshot (M09-001)
        ↓ explicit external transform + manifest
trainer records (outside Orc canonical storage)
        ↓ pinned ms-swift LoRA run, selected HF Qwen3 checkpoint + matching tokenizer
PEFT adapter + merged HF checkpoint + lineage
        ↓ pinned llama.cpp conversion and quantization
candidate GGUF + digest + conversion evidence
        ↓ existing ORC_QWEN3_GGUF / LocalInferenceRuntime boundary
M09-003 exact nine-capability report
        ↓ M09-004 fixed comparison gate
operator-authorized promotion decision
```

The first experiment must be offline after inputs are staged: model/tokenizer files, dataset artifact, source code revisions, lockfiles/wheels, and conversion tools are supplied before execution; training, export, conversion, candidate evaluation, and comparison make no network calls. Seeds, trainer settings, tokenizer/chat-template identity, maximum lengths, precision, adapter target modules, quantization type, CPU/GPU placement, and conversion revisions must be captured.

The M09-002 expected semantics remain authoritative. M09-003 remains the baseline/candidate report shape. M09-004 remains the only promotion comparator. Training failures, poor scores, or successful loss reduction must not mutate Orc state, create experience examples automatically, change the default model, or bypass operator authorization.

## Unresolved blockers and non-claims

- Backend selection is deferred because the actual dataset, model, hardware, and baseline are unavailable.
- No Qwen3-8B result is asserted for native llama.cpp training.
- No claim is made that Q4_K_M GGUF can be consumed directly by ms-swift, LlamaFactory, PEFT, or a full trainer.
- No claim is made that a Qwen3 PEFT adapter can be loaded directly by the current Orc runtime.
- No dataset size, capability balance, quality distribution, split, or training readiness is inferred from tests or schemas.
- No full fine-tune memory estimate beyond Qwen’s documented 4 × 60 GB example is asserted.
- No architecture decision is added and D-007 is unchanged; the conditional Python toolchain is isolated from the Rust/native runtime as D-007 already permits for the separate training/export concern.

## Non-destructive verification plan

Before any training or large download is approved:

1. Initialize/open the configured registry through the normal operator path, then call the existing read-only inventory and snapshot APIs and record totals and exact capability coverage. Treat a missing table as provisioning state, not an empty dataset.
2. Obtain explicit operator-approved, digest-verified `Qwen/Qwen3-8B` instruct-checkpoint weights and matching tokenizer at a pinned revision for the documented ms-swift path; treat `Qwen/Qwen3-8B-Base` as a separate pretraining checkpoint requiring separate qualification, not as an interchangeable substitute. Separately obtain or build the inference GGUF needed for M09-003.
3. Acquire a GPU environment meeting the pinned trainer’s preflight measurement. Re-run a tiny dry-run/preflight only after the trainer revision and dependency lock are reviewed.
4. Define and review the external snapshot-to-trainer transformation, including message/template semantics and immutable source manifest. Do not change M09-001 records.
5. Run one LoRA qualification with fixed seeds and bounded output artifacts. Preserve adapter, merged HF weights, logs, and lineage; do not promote.
6. Convert and quantize the merged checkpoint with pinned llama.cpp tooling, load it through the existing runtime, and run the exact M09-003 suite.
7. Compare candidate and baseline with M09-004. Only an exact passing comparison plus explicit operator authorization can support a later default-model task.

## Proposed independently reviewable follow-on tasks

These are intentionally narrow and remain unstarted:

1. **M09-006 — Provision and measure the canonical specialization source.** Verify the configured global registry, capture M08-011 inventory and M09-001 snapshot evidence, and resolve whether the current registry path is the intended operator data store. No export or mutation policy.
2. **M09-007 — Specify the external snapshot-to-trainer transform.** Define one versioned, reproducible message/record transform and manifest for the chosen trainer without changing canonical records, adding a readiness threshold, or selecting splits by assumption.
3. **M09-008 — Qualify one pinned Qwen3-8B LoRA toolchain.** Use a supplied GPU, the explicitly selected original HF checkpoint (`Qwen/Qwen3-8B` for the documented instruct path; `Qwen/Qwen3-8B-Base` only under separate qualification), a reviewed dependency lock, and a small controlled run; record peak memory, failures, seed controls, and adapter artifact format. No Orc runtime change.
4. **M09-009 — Package and verify a candidate GGUF.** Convert the merged HF checkpoint, quantize with explicit parameters, record digests/lineage, and prove loading through `LlamaCppRuntime`. No default change.
5. **M09-010 — Execute candidate evaluation and comparison.** Run M09-003 for the candidate, validate comparability, and apply the fixed M09-004 gate. No automatic promotion.

## Completion evidence

Inspected repository surfaces: `Cargo.toml`, `Cargo.lock`, `src/local_runtime.rs`, `src/local_runtime/llama_cpp.rs`, M08-011, M09-001 through M09-004, `DECISIONS.md`, `ARCHITECTURE.md`, `STATUS.md`, `ROADMAP.md`, the task index, `.orc/orc.db`, and the configured global registry file. Inspected actual CPU/RAM/GPU/tool/model/dataset state without writing files, opening the application, installing toolchains, or downloading weights.

Upstream capability and format evidence is linked in the tables above. Native llama.cpp training was not treated as equivalent to inference support. Qwen3 instruct/base/adapter/GGUF boundaries, missing local readiness, conditional recommendation, unresolved blockers, and follow-on task dependencies are explicit.

Changed documentation: this report; `tasks/M09-005.md`; `tasks/README.md`; `STATUS.md`; and `ROADMAP.md`. No production code, dependency, model, dataset, database, default model, evaluation expectation, or architecture decision changed. Independent review remains required before committing or pushing this documentation task.
