#!/usr/bin/env python3
import argparse
import json
import os
import re
import struct
from pathlib import Path

# Adjust these if your tensor names differ.
# This matches the common Qwen MoE naming pattern used by flash-moe-style code.
EXPERT_PATTERNS = {
    "gate_proj.weight": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.gate_proj\.weight$"
    ),
    "gate_proj.scales": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.gate_proj\.scales$"
    ),
    "gate_proj.biases": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.gate_proj\.biases$"
    ),

    "up_proj.weight": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.up_proj\.weight$"
    ),
    "up_proj.scales": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.up_proj\.scales$"
    ),
    "up_proj.biases": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.up_proj\.biases$"
    ),

    "down_proj.weight": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.down_proj\.weight$"
    ),
    "down_proj.scales": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.down_proj\.scales$"
    ),
    "down_proj.biases": re.compile(
        r"^language_model\.model\.layers\.(\d+)\.mlp\.switch_mlp\.down_proj\.biases$"
    ),
}

# Fallback patterns in case the model stores experts under experts.0 / experts.1 style names.
FALLBACK_PATTERNS = {
    "gate_proj.weight": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.gate_proj\.weight$"),
    "gate_proj.scales": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.gate_proj\.scales$"),
    "gate_proj.biases": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.gate_proj\.biases$"),
    "up_proj.weight": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.up_proj\.weight$"),
    "up_proj.scales": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.up_proj\.scales$"),
    "up_proj.biases": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.up_proj\.biases$"),
    "down_proj.weight": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.down_proj\.weight$"),
    "down_proj.scales": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.down_proj\.scales$"),
    "down_proj.biases": re.compile(r"^model\.layers\.(\d+)\.mlp\.experts\.(\d+)\.down_proj\.biases$"),
}


def read_safetensors_header(path: Path):
    with open(path, "rb") as f:
        header_len = struct.unpack("<Q", f.read(8))[0]
        header = json.loads(f.read(header_len))
    return header, 8 + header_len


def build_header_cache(model_path: Path, shard_files):
    cache = {}
    for shard in sorted(shard_files):
        shard_path = model_path / shard
        header, data_start = read_safetensors_header(shard_path)
        cache[shard] = {
            "header": header,
            "data_start": data_start,
        }
    return cache


def get_tensor_meta(header_cache, shard_name, tensor_name):
    meta = header_cache[shard_name]["header"][tensor_name]
    data_start = header_cache[shard_name]["data_start"]

    # safetensors data_offsets are relative to the data section
    rel_begin, rel_end = meta["data_offsets"]
    abs_begin = data_start + rel_begin
    abs_end = data_start + rel_end
    size_bytes = abs_end - abs_begin
    shape = meta.get("shape", [])
    dtype = meta.get("dtype", "")

    return {
        "abs_begin": abs_begin,
        "abs_end": abs_end,
        "size_bytes": size_bytes,
        "shape": shape,
        "dtype": dtype,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model-path", required=True, help="HF snapshot dir")
    ap.add_argument("--out", default="expert_index_35b.json")
    args = ap.parse_args()

    model_path = Path(args.model_path).expanduser().resolve()
    index_path = model_path / "model.safetensors.index.json"

    if not index_path.exists():
        raise FileNotFoundError(f"Missing {index_path}")

    with open(index_path) as f:
        index_json = json.load(f)

    weight_map = index_json["weight_map"]
    shard_files = sorted(set(weight_map.values()))
    header_cache = build_header_cache(model_path, shard_files)

    expert_reads = {}
    matched_any = False

    # First try "packed experts tensor" style:
    for tensor_name, shard_name in weight_map.items():
        for comp_name, pat in EXPERT_PATTERNS.items():
            m = pat.match(tensor_name)
            if not m:
                continue

            matched_any = True
            layer = m.group(1)
            meta = get_tensor_meta(header_cache, shard_name, tensor_name)

            print(
                f"matched layer={layer} comp={comp_name} "
                f"tensor={tensor_name} shape={meta['shape']} dtype={meta['dtype']} size={meta['size_bytes']}"
            )

            # For packed-per-layer expert tensors, shape[0] should be num_experts
            if len(meta["shape"]) < 1:
                raise ValueError(f"Unexpected shape for {tensor_name}: {meta['shape']}")

            num_experts = meta["shape"][0]
            if num_experts <= 0:
                raise ValueError(f"Bad num_experts for {tensor_name}: {meta['shape']}")

            if meta["size_bytes"] % num_experts != 0:
                raise ValueError(
                    f"Tensor size not divisible by expert count for {tensor_name}: "
                    f"{meta['size_bytes']} / {num_experts}"
                )

            expert_stride = meta["size_bytes"] // num_experts
            expert_size = expert_stride

            expert_reads.setdefault(layer, {})[comp_name] = {
                "file": shard_name,
                "abs_offset": meta["abs_begin"],
                "expert_stride": expert_stride,
                "expert_size": expert_size,
                "tensor_name": tensor_name,
                "shape": meta["shape"],
                "dtype": meta["dtype"],
            }

    # Fallback: one tensor per expert
    if not matched_any:
        temp = {}
        for tensor_name, shard_name in weight_map.items():
            for comp_name, pat in FALLBACK_PATTERNS.items():
                m = pat.match(tensor_name)
                if not m:
                    continue

                layer, expert_idx = m.group(1), int(m.group(2))
                meta = get_tensor_meta(header_cache, shard_name, tensor_name)

                temp.setdefault(layer, {}).setdefault(comp_name, []).append(
                    {
                        "expert_idx": expert_idx,
                        "file": shard_name,
                        "abs_begin": meta["abs_begin"],
                        "size_bytes": meta["size_bytes"],
                        "tensor_name": tensor_name,
                        "shape": meta["shape"],
                        "dtype": meta["dtype"],
                    }
                )

        if not temp:
            raise RuntimeError(
                "Could not find expert tensors. Inspect tensor names in model.safetensors.index.json "
                "and adjust the regex patterns in this script."
            )

        # Convert list of per-expert tensors into flash-moe's base+stride form
        for layer, comps in temp.items():
            expert_reads[layer] = {}
            for comp_name, items in comps.items():
                items = sorted(items, key=lambda x: x["expert_idx"])

                first = items[0]
                stride = None
                if len(items) > 1:
                    stride = items[1]["abs_begin"] - items[0]["abs_begin"]
                else:
                    stride = first["size_bytes"]

                # Sanity checks
                expected_size = first["size_bytes"]
                for i, item in enumerate(items):
                    if item["size_bytes"] != expected_size:
                        raise ValueError(f"Mixed sizes in {layer}/{comp_name}")
                    if item["expert_idx"] != i:
                        raise ValueError(
                            f"Experts not contiguous for {layer}/{comp_name}: got {item['expert_idx']} at slot {i}"
                        )

                expert_reads[layer][comp_name] = {
                    "file": first["file"],
                    "abs_offset": first["abs_begin"],
                    "expert_stride": stride,
                    "expert_size": expected_size,
                    "tensor_name": first["tensor_name"],
                    "shape": first["shape"],
                    "dtype": first["dtype"],
                }

    out = {
        "model_path": str(model_path),
        "expert_reads": expert_reads,
    }

    with open(args.out, "w") as f:
        json.dump(out, f, indent=2)

    print(f"Wrote {args.out}")
    print(f"Layers found: {len(expert_reads)}")
    sample_layer = sorted(expert_reads.keys(), key=int)[0]
    print(f"Sample layer: {sample_layer}")
    print(json.dumps(expert_reads[sample_layer], indent=2))


if __name__ == "__main__":
    main()