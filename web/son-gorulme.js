// Son görülme, Nostr röleleri üzerinden.
// Sunucumuz olmadığı için "en son ne zaman açıktı" bilgisi eskiden sadece aynı anda açık
// olanlar arasında geçiyordu: biri biz kapalıyken girip çıkınca bunu bilemiyorduk.
// Şimdi herkes açıkken birkaç dakikada bir rölelere kısa bir not bırakır, açan da okur.
//
// - Not, davet kodundan türetilen anahtarla şifrelenir (AES-GCM). Röle ve davet kodu
//   olmayanlar içini okuyamaz; değiştirilmiş not açılmaz, yok sayılır.
// - Not "değiştirilebilir" türdedir (kind 30078, NIP-01): her kişinin rölede tek notu
//   durur, yenisi eskisinin yerine geçer. Kişinin Nostr anahtarı bu cihazda saklanır.
// - Röle notu silerse bir şey bozulmaz: kişi tekrar açınca yenisi yazılır.

import {schnorr, utils} from './vendor/noble-secp256k1-3.2.0.js'

const ROLELER = ['wss://relay.damus.io', 'wss://nos.lol', 'wss://relay.primal.net', 'wss://nostr.mom']
const TUR = 30078
const YAZMA_ARALIGI = 3 * 60e3

const metin = new TextEncoder()
const hex = b => Array.from(b, x => x.toString(16).padStart(2, '0')).join('')
const hexten = h => Uint8Array.from(h.match(/../g), x => parseInt(x, 16))
const ozet = async veri => new Uint8Array(await crypto.subtle.digest('SHA-256', typeof veri === 'string' ? metin.encode(veri) : veri))
const b64 = b => btoa(String.fromCharCode(...b))
const b64ten = s => Uint8Array.from(atob(s), c => c.charCodeAt(0))

// Bu cihazın Nostr anahtarı (sadece bu notları imzalamak için; kimlik değil).
function gizliAnahtar() {
  let h = null
  try { h = localStorage.getItem('nostrAnahtar') } catch {}
  if (!/^[0-9a-f]{64}$/.test(h || '')) {
    h = hex(utils.randomSecretKey())
    try { localStorage.setItem('nostrAnahtar', h) } catch {}
  }
  return hexten(h)
}

// davet: davet kodu. benimId: kendi userId'miz. al(userId, zaman): başkasının notu gelince.
// gunluk(...): ses.log'a yazmak için.
export async function sonGorulmeBaslat({davet, benimId, al, gunluk = () => {}}) {
  const etiket = hex(await ozet('monke-son-gorulme-etiket|' + davet))
  const anahtar = await crypto.subtle.importKey('raw', await ozet('monke-son-gorulme-anahtar|' + davet),
    'AES-GCM', false, ['encrypt', 'decrypt'])
  let gizli = gizliAnahtar(), acik
  try { acik = hex(schnorr.getPublicKey(gizli)) } catch {
    try { localStorage.removeItem('nostrAnahtar') } catch {}
    gizli = gizliAnahtar(); acik = hex(schnorr.getPublicKey(gizli))
  }
  const abone = 'g' + hex(crypto.getRandomValues(new Uint8Array(6)))
  const soketler = new Map()
  let bitti = false, sonNot = null, zamanlayici = 0

  async function notYap() {
    const iv = crypto.getRandomValues(new Uint8Array(12))
    const sifreli = new Uint8Array(await crypto.subtle.encrypt({name: 'AES-GCM', iv}, anahtar,
      metin.encode(JSON.stringify({u: benimId, z: Date.now()}))))
    const olay = {pubkey: acik, created_at: Math.floor(Date.now() / 1000), kind: TUR,
      tags: [['d', etiket]], content: b64(iv) + '.' + b64(sifreli)}
    const id = await ozet(JSON.stringify([0, olay.pubkey, olay.created_at, olay.kind, olay.tags, olay.content]))
    olay.id = hex(id)
    olay.sig = hex(await schnorr.signAsync(id, gizli))
    return olay
  }

  async function notAc(olay) {
    try {
      if (olay?.kind !== TUR || olay.pubkey === acik || typeof olay.content !== 'string') return
      if (!olay.tags?.some(t => t[0] === 'd' && t[1] === etiket)) return
      const [iv, veri] = olay.content.split('.').map(b64ten)
      const n = JSON.parse(new TextDecoder().decode(await crypto.subtle.decrypt({name: 'AES-GCM', iv}, anahtar, veri)))
      if (typeof n?.u !== 'string' || !n.u || n.u.length > 200 || n.u === benimId || !Number.isFinite(n.z)) return
      al(n.u, Math.min(n.z, Date.now()))
    } catch {}  // açılmayan not (başka biri / bozuk) yok sayılır
  }

  function baglan(url, bekle = 5e3) {
    if (bitti) return
    let s
    try { s = new WebSocket(url) } catch { return }
    soketler.set(url, s)
    s.onopen = () => {
      bekle = 5e3
      s.send(JSON.stringify(['REQ', abone, {kinds: [TUR], '#d': [etiket], limit: 50}]))
      if (sonNot) s.send(JSON.stringify(['EVENT', sonNot]))
    }
    s.onmessage = e => {
      let m
      try { m = JSON.parse(e.data) } catch { return }
      if (m[0] === 'EVENT' && m[1] === abone) notAc(m[2])
      else if (m[0] === 'OK' && m[2] === false) gunluk('son görülme notu reddedildi:', url, String(m[3]).slice(0, 80))
    }
    s.onclose = () => {
      if (soketler.get(url) === s) soketler.delete(url)
      if (!bitti) setTimeout(() => baglan(url, Math.min(bekle * 2, 120e3)), bekle)
    }
    s.onerror = () => {}
  }

  async function yaz() {
    if (bitti) return
    try {
      sonNot = await notYap()
      const t = JSON.stringify(['EVENT', sonNot])
      for (const s of soketler.values()) if (s.readyState === 1) s.send(t)
    } catch (e) { gunluk('son görülme notu yazılamadı:', e?.message || e) }
  }

  for (const url of ROLELER) baglan(url)
  await yaz()
  zamanlayici = setInterval(yaz, YAZMA_ARALIGI)
  // Kapatırken son bir not (gitmeyebilir; gitmezse en fazla 3 dk eski kalır).
  const kapanirken = () => { yaz() }
  addEventListener('pagehide', kapanirken)

  return {
    yaz,
    async kapat() {
      await yaz()
      bitti = true
      clearInterval(zamanlayici)
      removeEventListener('pagehide', kapanirken)
      for (const s of soketler.values()) {
        try { s.send(JSON.stringify(['CLOSE', abone])) } catch {}
        // Son not gitsin diye biraz beklenir.
        setTimeout(() => { try { s.close() } catch {} }, 1500)
      }
      soketler.clear()
    },
  }
}
