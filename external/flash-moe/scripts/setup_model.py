#!/usr/bin/env python3
import json
import struct
import sys
import os
import argparse
import time
from pathlib import Path
from collections import defaultdict
import re

def parse_safetensors_header(filepath):
    with open(filepath, 'rb') as f:
        header_len = struct.unpack('<Q', f.read(8))[0]
        header = json.loads(f.read(header_len))
        data_start = 8 + header_len
    return header, data_start

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--repo', type=str, help='HuggingFace repo to download from')
    parser.add_argument('--model', type=str, help='Local path to model directory')
    parser.add_argument('--output', type=str, required=True, help='Output directory')
    args = parser.parse_args()

    if args.repo:
        try:
            from huggingface_hub import snapshot_download
        except ImportError:
            print("Downloading requires huggingface_hub. Run: pip install huggingface_hub", file=sys.stderr)
            sys.exit(1)
        # Using a custom tqdm to print progress safely to our Rust parser
        print(f"Downloading {args.repo}...")
        hub_cache = os.environ.get("HUGGINGFACE_HUB_CACHE")
        download_kw = {"repo_id": args.repo}
        if hub_cache:
            download_kw["cache_dir"] = hub_cache
        model_path = Path(snapshot_download(**download_kw))
    elif args.model:
        model_path = Path(args.model)
    else:
        print("Must specify either --repo or --model", file=sys.stderr)
        sys.exit(1)

    output_dir = Path(args.output)
    output_dir.mkdir(parents=True, exist_ok=True)
    packed_experts_dir = output_dir / "packed_experts"
    packed_experts_dir.mkdir(parents=True, exist_ok=True)

    with open(model_path / 'config.json') as f:
        config = json.load(f)

    index_path = model_path / 'model.safetensors.index.json'
    with open(index_path) as f:
        idx = json.load(f)
    weight_map = idx['weight_map']

    num_layers = config.get("num_hidden_layers", 48)
    num_experts = config.get("num_experts", 128)
    moe_intermediate = config.get("moe_intermediate_size", 1024)
    hidden_size = config.get("hidden_size", 2048)

    expert_pattern = re.compile(r'\.switch_mlp\.(gate_proj|up_proj|down_proj)\.(weight|scales|biases)$')

    tensors_to_extract = {}
    experts_to_pack = defaultdict(dict)

    for name, filename in weight_map.items():
        if "vision_tower" in name or "model.visual" in name:
            continue
            
        match = expert_pattern.search(name)
        if match:
            m = re.match(r'model\.layers\.(\d+)\.', name)
            layer_idx = int(m.group(1))
            comp_name = match.group(1) + "." + match.group(2)
            experts_to_pack[layer_idx][comp_name] = {"file": filename, "name": name}
        else:
            tensors_to_extract[name] = filename

    # Compute Expert Packing Sizes
    gate_up_weight_size = moe_intermediate * (hidden_size // 8) * 4
    gate_up_scale_size  = moe_intermediate * (hidden_size // 64) * 2
    down_weight_size    = hidden_size * (moe_intermediate // 8) * 4
    down_scale_size     = hidden_size * (moe_intermediate // 64) * 2

    COMPONENTS = []
    offset = 0
    for name in ["gate_proj.weight", "gate_proj.scales", "gate_proj.biases",
                 "up_proj.weight", "up_proj.scales", "up_proj.biases",
                 "down_proj.weight", "down_proj.scales", "down_proj.biases"]:
        is_down = "down" in name
        is_weight = "weight" in name
        if is_weight:
            size = down_weight_size if is_down else gate_up_weight_size
        else:
            size = down_scale_size if is_down else gate_up_scale_size
            
        COMPONENTS.append({"name": name, "offset": offset, "size": size})
        offset += size
        
    EXPERT_SIZE = offset
    LAYER_SIZE = num_experts * EXPERT_SIZE

    print("Extracting: 0%")
    sys.stdout.flush()

    all_tensors = []
    by_file = defaultdict(list)
    for name, filename in tensors_to_extract.items():
        san_name = name[15:] if name.startswith("language_model.") else name
        all_tensors.append((san_name, name, filename))
        by_file[filename].append(name)
        
    header_cache = {}
    for filename in set(by_file.keys()):
        header_cache[filename] = parse_safetensors_header(str(model_path / filename))

    bin_path = output_dir / 'model_weights.bin'
    manifest = {
        "model": str(model_path),
        "num_tensors": len(all_tensors),
        "tensors": {},
        "config": {
            "hidden_size": hidden_size,
            "num_hidden_layers": num_layers,
            "num_attention_heads": config.get("num_attention_heads", 16),
            "num_key_value_heads": config.get("num_key_value_heads", 2),
            "head_dim": config.get("head_dim", hidden_size // config.get("num_attention_heads", 16) if config.get("num_attention_heads") else 64),
            "vocab_size": config.get("vocab_size", 151936),
            "rms_norm_eps": config.get("rms_norm_eps", 1e-6),
            "num_experts": num_experts,
            "num_experts_per_tok": config.get("num_experts_per_tok", 8),
            "moe_intermediate_size": moe_intermediate,
            "shared_expert_intermediate_size": config.get("shared_expert_intermediate_size", 0),
        }
    }

    # Some Qwen architectures have pure full_attention
    is_hybrid = num_layers == 40 and num_experts == 256
    layer_types = []
    for i in range(num_layers):
        if is_hybrid:
            layer_types.append("full_attention" if (i + 1) % 4 == 0 else "linear_attention")
        else:
            layer_types.append("full_attention")
    manifest["config"]["layer_types"] = layer_types

    offset = 0
    ALIGN = 64
    all_tensors.sort()
    total_tensors = len(all_tensors)
    
    with open(bin_path, 'wb') as out_f:
        for idx, (san_name, orig_name, filename) in enumerate(all_tensors):
            header, data_start = header_cache[filename]
            meta = header[orig_name]
            tensor_offsets = meta['data_offsets']
            byte_len = tensor_offsets[1] - tensor_offsets[0]
            if offset % ALIGN != 0:
                pad = ALIGN - (offset % ALIGN)
                out_f.write(b'\x00' * pad)
                offset += pad

            with open(model_path / filename, 'rb') as sf:
                sf.seek(data_start + tensor_offsets[0])
                data = sf.read(byte_len)
            out_f.write(data)

            manifest["tensors"][san_name] = {
                "offset": offset,
                "size": byte_len,
                "shape": meta['shape'],
                "dtype": meta['dtype']
            }
            offset += byte_len
            
            if (idx + 1) % 10 == 0 or idx == total_tensors - 1:
                pct = int((idx + 1) / total_tensors * 100)
                print(f"Extracting: {pct}%")
                sys.stdout.flush()

    with open(output_dir / 'model_weights.json', 'w') as f:
        json.dump(manifest, f, indent=2)

    # Collect open file descriptors for packing experts
    expert_fds = {}
    for layer in experts_to_pack.values():
        for info in layer.values():
            filename = info["file"]
            if filename not in expert_fds:
                expert_fds[filename] = os.open(model_path / filename, os.O_RDONLY)

    for layer_idx in range(num_layers):
        print(f"Packing layer {layer_idx + 1}/{num_layers}")
        sys.stdout.flush()
        
        if layer_idx not in experts_to_pack:
            continue
            
        out_path = packed_experts_dir / f"layer_{layer_idx:02d}.bin"
        fd_out = os.open(out_path, os.O_RDWR | os.O_CREAT | os.O_TRUNC, 0o644)
        os.ftruncate(fd_out, LAYER_SIZE)
        
        layer_info = experts_to_pack[layer_idx]
        
        read_plan = []
        for expert_idx in range(num_experts):
            for comp in COMPONENTS:
                comp_name = comp['name']
                orig_name = layer_info[comp_name]['name']
                filename = layer_info[comp_name]['file']
                
                header, data_start = header_cache[filename]
                meta = header[orig_name]
                abs_offset = data_start + meta['data_offsets'][0]
                
                src_fd = expert_fds[filename]
                # Stride logic: expert array is outer-most dimension [num_experts, ...]
                src_offset = abs_offset + expert_idx * comp['size']
                dst_offset = expert_idx * EXPERT_SIZE + comp['offset']
                
                read_plan.append((src_fd, src_offset, dst_offset, comp['size']))

        read_plan.sort(key=lambda x: (x[0], x[1]))
        
        for src_fd, src_offset, dst_offset, size in read_plan:
            data = os.pread(src_fd, size, src_offset)
            os.pwrite(fd_out, data, dst_offset)
            
        os.close(fd_out)

    for fd in expert_fds.values():
        os.close(fd)

    config["layer_types"] = layer_types
    with open(output_dir / 'config.json', 'w') as f:
        json.dump(config, f, indent=2)

    print("Complete")
    sys.stdout.flush()

if __name__ == '__main__':
    main()
