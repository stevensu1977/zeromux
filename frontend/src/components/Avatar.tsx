import { useState } from 'react'

interface Props {
  src: string | null | undefined
  login: string
  size?: number
}

// GitHub avatar with graceful degradation: when avatars.githubusercontent.com
// is unreachable (network partition, blocked), fall back to an initial badge
// instead of leaving a hung image request / broken icon.
export default function Avatar({ src, login, size = 20 }: Props) {
  const [failed, setFailed] = useState(false)
  const initial = (login || '?').charAt(0).toUpperCase()

  if (!src || failed) {
    return (
      <span
        style={{ width: size, height: size, fontSize: size * 0.5 }}
        className="rounded-full shrink-0 bg-[var(--accent-blue)] text-white flex items-center justify-center font-semibold select-none"
      >
        {initial}
      </span>
    )
  }
  return (
    <img
      src={src}
      alt=""
      width={size}
      height={size}
      loading="lazy"
      onError={() => setFailed(true)}
      className="rounded-full shrink-0"
      style={{ width: size, height: size }}
    />
  )
}
