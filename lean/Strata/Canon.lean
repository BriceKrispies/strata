namespace Strata

/-- Canonical value, mirroring a Rust `Value` enum serialised by serde's default
externally tagged form. -/
inductive CanonValue where
  | null
  | int (i : Int)
  | str (s : String)
  | bool (b : Bool)
  | ref (id : Nat)
  | list (vs : List CanonValue)
  deriving Repr

/-- Lowercase hex digit for `n < 16`. -/
def canonHexDigit (n : Nat) : Char :=
  if n < 10 then Char.ofNat (48 + n) else Char.ofNat (87 + n)

/-- JSON escape of a single character, as serde_json writes it. -/
def canonEscapeChar (c : Char) : String :=
  if c == '"' then "\\\""
  else if c == '\\' then "\\\\"
  else if c.toNat == 8 then "\\b"
  else if c.toNat == 12 then "\\f"
  else if c == '\n' then "\\n"
  else if c == '\r' then "\\r"
  else if c == '\t' then "\\t"
  else if c.toNat < 0x20 then
    "\\u00" ++ String.singleton (canonHexDigit (c.toNat / 16))
      ++ String.singleton (canonHexDigit (c.toNat % 16))
  else String.singleton c

/-- JSON escape of a string body (without the surrounding quotes). -/
def canonEscapeStr (s : String) : String :=
  s.foldl (fun acc c => acc ++ canonEscapeChar c) ""

-- Canonical JSON text: no whitespace, sorted keys (one key per object here).
mutual
/-- Canonical JSON text of a value. -/
def canonJson : CanonValue → String
  | .null => "\"Null\""
  | .int i => "{\"Int\":" ++ toString i ++ "}"
  | .str s => "{\"Str\":\"" ++ canonEscapeStr s ++ "\"}"
  | .bool b => "{\"Bool\":" ++ (if b then "true" else "false") ++ "}"
  | .ref n => "{\"Ref\":" ++ toString n ++ "}"
  | .list vs => "{\"List\":[" ++ canonJsonList vs ++ "]}"

/-- Comma-separated canonical JSON of list elements. -/
def canonJsonList : List CanonValue → String
  | [] => ""
  | v :: vs => canonJson v ++ (if vs.isEmpty then "" else ",") ++ canonJsonList vs
end

/-! SHA-256 (FIPS 180-4) -/

/-- Round constants K[0..63]. -/
def sha256K : Array UInt32 := #[
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2]

/-- Initial hash value H0..H7. -/
def sha256H0 : Array UInt32 :=
  #[0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]

/-- Right rotation of a 32-bit word. `n` must be in 1..31. -/
def sha256Rotr (x : UInt32) (n : UInt32) : UInt32 :=
  (x >>> n) ||| (x <<< (32 - n))

def sha256BigSigma0 (x : UInt32) : UInt32 :=
  sha256Rotr x 2 ^^^ sha256Rotr x 13 ^^^ sha256Rotr x 22

def sha256BigSigma1 (x : UInt32) : UInt32 :=
  sha256Rotr x 6 ^^^ sha256Rotr x 11 ^^^ sha256Rotr x 25

def sha256SmallSigma0 (x : UInt32) : UInt32 :=
  sha256Rotr x 7 ^^^ sha256Rotr x 18 ^^^ (x >>> 3)

def sha256SmallSigma1 (x : UInt32) : UInt32 :=
  sha256Rotr x 17 ^^^ sha256Rotr x 19 ^^^ (x >>> 10)

def sha256Ch (e f g : UInt32) : UInt32 :=
  (e &&& f) ^^^ ((~~~e) &&& g)

def sha256Maj (a b c : UInt32) : UInt32 :=
  (a &&& b) ^^^ (a &&& c) ^^^ (b &&& c)

/-- Message padding: 0x80, zeros, then the 64-bit big-endian bit length. -/
def sha256Pad (msg : ByteArray) : ByteArray := Id.run do
  let len := msg.size
  let zeros := (119 - len % 64) % 64
  let mut out := msg.push 0x80
  for _ in [0:zeros] do
    out := out.push 0
  let bits : UInt64 := len.toUInt64 * 8
  for i in [0:8] do
    out := out.push (bits >>> (8 * (7 - i)).toUInt64).toUInt8
  return out

/-- Big-endian 32-bit word read at byte offset `off`. -/
def sha256WordAt (buf : ByteArray) (off : Nat) : UInt32 :=
  let b0 := (buf.get! off).toUInt32
  let b1 := (buf.get! (off + 1)).toUInt32
  let b2 := (buf.get! (off + 2)).toUInt32
  let b3 := (buf.get! (off + 3)).toUInt32
  (b0 <<< 24) ||| (b1 <<< 16) ||| (b2 <<< 8) ||| b3

/-- Compress one 64-byte block starting at `off` into the hash state `h`. -/
def sha256Block (h : Array UInt32) (buf : ByteArray) (off : Nat) : Array UInt32 := Id.run do
  let mut w : Array UInt32 := (List.replicate 64 (0 : UInt32)).toArray
  for t in [0:16] do
    w := w.set! t (sha256WordAt buf (off + 4 * t))
  for t in [16:64] do
    let s0 := sha256SmallSigma0 (w[t - 15]!)
    let s1 := sha256SmallSigma1 (w[t - 2]!)
    w := w.set! t (s1 + w[t - 7]! + s0 + w[t - 16]!)
  let mut a := h[0]!
  let mut b := h[1]!
  let mut c := h[2]!
  let mut d := h[3]!
  let mut e := h[4]!
  let mut f := h[5]!
  let mut g := h[6]!
  let mut hh := h[7]!
  for t in [0:64] do
    let t1 := hh + sha256BigSigma1 e + sha256Ch e f g + sha256K[t]! + w[t]!
    let t2 := sha256BigSigma0 a + sha256Maj a b c
    hh := g
    g := f
    f := e
    e := d + t1
    d := c
    c := b
    b := a
    a := t1 + t2
  return #[h[0]! + a, h[1]! + b, h[2]! + c, h[3]! + d,
           h[4]! + e, h[5]! + f, h[6]! + g, h[7]! + hh]

/-- Append a 32-bit word as four big-endian bytes. -/
def sha256WordPush (acc : ByteArray) (x : UInt32) : ByteArray :=
  let a1 := acc.push (x >>> 24).toUInt8
  let a2 := a1.push (x >>> 16).toUInt8
  let a3 := a2.push (x >>> 8).toUInt8
  a3.push x.toUInt8

/-- SHA-256 digest of a byte array (32 bytes). -/
def sha256Bytes (msg : ByteArray) : ByteArray :=
  let padded := sha256Pad msg
  let nBlocks := padded.size / 64
  let hFinal := (List.range nBlocks).foldl
    (fun h blk => sha256Block h padded (64 * blk)) sha256H0
  hFinal.foldl sha256WordPush ByteArray.empty

/-- Lowercase hex digit pair for one byte. -/
def sha256HexByte (acc : String) (b : UInt8) : String :=
  let hi := canonHexDigit (b.toNat / 16)
  let lo := canonHexDigit (b.toNat % 16)
  (acc.push hi).push lo

/-- Lowercase hex SHA-256 of the UTF-8 bytes of `s`. -/
def sha256Hex (s : String) : String :=
  (sha256Bytes s.toUTF8).toList.foldl sha256HexByte ""

/-- Hash key of a canonical value. -/
def runKeyHash (v : CanonValue) : String :=
  sha256Hex (canonJson v)

end Strata
