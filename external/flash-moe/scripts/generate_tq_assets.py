import os
import struct
import numpy as np
from scipy import integrate, special
import torch

def beta_coord_pdf(x: np.ndarray, d: int) -> np.ndarray:
    x = np.clip(x, -1 + 1e-15, 1 - 1e-15)
    log_const = (
        special.gammaln(d / 2.0)
        - 0.5 * np.log(np.pi)
        - special.gammaln((d - 1) / 2.0)
    )
    exponent = (d - 3) / 2.0
    return np.exp(log_const + exponent * np.log(1 - x**2))

def conditional_mean(lo: float, hi: float, d: int) -> float:
    num, _ = integrate.quad(lambda x: x * beta_coord_pdf(np.array([x]), d)[0], lo, hi)
    den, _ = integrate.quad(lambda x: beta_coord_pdf(np.array([x]), d)[0], lo, hi)
    if den < 1e-30:
        return 0.5 * (lo + hi)
    return num / den

def lloyd_max_codebook(d: int, bits: int, max_iter: int = 200, tol: float = 1e-12):
    k = 2 ** bits
    centroids = np.linspace(-0.95, 0.95, k)

    for _ in range(max_iter):
        old = centroids.copy()
        boundaries = np.empty(k + 1)
        boundaries[0] = -1.0
        boundaries[-1] = 1.0
        boundaries[1:-1] = 0.5 * (centroids[:-1] + centroids[1:])

        for i in range(k):
            centroids[i] = conditional_mean(boundaries[i], boundaries[i + 1], d)

        if np.max(np.abs(centroids - old)) < tol:
            break

    boundaries = np.empty(k + 1)
    boundaries[0] = -1.0
    boundaries[-1] = 1.0
    boundaries[1:-1] = 0.5 * (centroids[:-1] + centroids[1:])
    return centroids.astype(np.float32), boundaries.astype(np.float32)

def generate_rotation_matrix(d: int, seed: int = 42):
    g = torch.Generator(device="cpu")
    g.manual_seed(seed)
    A = torch.randn(d, d, generator=g, dtype=torch.float32)
    Q, R = torch.linalg.qr(A)
    sign = torch.sign(torch.diag(R))
    Q = Q * sign.unsqueeze(0)
    return Q.numpy().astype(np.float32)

def generate_qjl_matrix(d: int, seed: int = 12345):
    g = torch.Generator(device="cpu")
    g.manual_seed(seed)
    S = torch.randn(d, d, generator=g, dtype=torch.float32)
    return S.numpy().astype(np.float32)

def main():
    head_dim = 256
    # For TurboQuantProd, the MSE part uses (b-1) bits. 
    # With 4-bit total, MSE uses 3-bit (8 centroids). 
    # Wait, the reference plan uses 4-bit total (3-bit MSE + 1-bit QJL) OR 4-bit MSE.
    # The reference says: "Combined (3b key + 2b val)" or "3-bit". 
    # We will use 4-bit total unpacked to uint8 -> 3-bit MSE (8 centroids) + 1-bit QJL (per dim).
    # This means MSE codebook needs 2^3 = 8 centroids.
    mse_bits = 3 
    
    print(f"Generating Lloyd-Max {mse_bits}-bit Codebook for d={head_dim}...")
    centroids, boundaries = lloyd_max_codebook(head_dim, mse_bits)
    
    # We only need one Pi and S matrix shared across the entire model/layers to save memory.
    # The paper allows per-layer, but shared is simpler and extremely effective.
    print(f"Generating Rotation matrix Pi ({head_dim}x{head_dim})...")
    Pi = generate_rotation_matrix(head_dim, seed=42)
    
    print(f"Generating QJL Matrix S ({head_dim}x{head_dim})...")
    S = generate_qjl_matrix(head_dim, seed=12345)
    
    out_path = os.path.join(os.path.dirname(__file__), "..", "tq_assets.bin")
    
    # Binary Format:
    # [Magic: uint32] = 0x54514E54 ('TQNT')
    # [Version: uint32] = 1
    # [HeadDim: uint32] = 256
    # [MseBits: uint32] = 3
    # [Centroids: float32 * 2^mse_bits] = 8 * 4 = 32 bytes
    # [Boundaries: float32 * (2^mse_bits + 1)] = 9 * 4 = 36 bytes
    # [Pi: float32 * d * d] = 256*256*4 = 262144 bytes
    # [S: float32 * d * d] = 256*256*4 = 262144 bytes
    
    magic = 0x54514E54
    version = 1
    
    with open(out_path, "wb") as f:
        f.write(struct.pack("<4I", magic, version, head_dim, mse_bits))
        f.write(centroids.tobytes())
        f.write(boundaries.tobytes())
        f.write(Pi.tobytes())
        f.write(S.tobytes())
        
    print(f"Successfully generated {out_path} ({os.path.getsize(out_path)} bytes)")

if __name__ == "__main__":
    main()
