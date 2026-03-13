export function CarlaLogo({
  className,
  compact = false,
}: {
  className?: string;
  compact?: boolean;
}) {
  return (
    <svg
      className={className}
      viewBox={compact ? "0 0 96 96" : "0 0 260 96"}
      role="img"
      aria-label="Carla logo"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
    >
      <defs>
        <linearGradient id="carlaBadge" x1="12" y1="10" x2="82" y2="86" gradientUnits="userSpaceOnUse">
          <stop stopColor="#FFD36A" />
          <stop offset="0.5" stopColor="#FF8F45" />
          <stop offset="1" stopColor="#FF5C6C" />
        </linearGradient>
        <linearGradient id="carlaLetter" x1="32" y1="28" x2="66" y2="68" gradientUnits="userSpaceOnUse">
          <stop stopColor="#FFF7E1" />
          <stop offset="1" stopColor="#FFE0A2" />
        </linearGradient>
      </defs>

      <rect x="8" y="8" width="80" height="80" rx="28" fill="url(#carlaBadge)" />
      <path
        d="M49.919 28C38.383 28 29 37.193 29 48.5C29 59.807 38.383 69 49.919 69C56.721 69 62.949 65.866 66.902 60.585L60.258 55.932C57.862 59.078 54.114 60.938 49.919 60.938C42.885 60.938 37.163 55.332 37.163 48.5C37.163 41.668 42.885 36.062 49.919 36.062C54.135 36.062 57.892 37.936 60.284 41.103L66.894 36.39C62.951 31.12 56.714 28 49.919 28Z"
        fill="url(#carlaLetter)"
      />

      {!compact ? (
        <g>
          <text
            x="110"
            y="61"
            fill="#F9F2E7"
            fontSize="38"
            fontWeight="700"
            fontFamily="IBM Plex Sans, Avenir Next, sans-serif"
            letterSpacing="0.02em"
          >
            Carla
          </text>
        </g>
      ) : null}
    </svg>
  );
}
