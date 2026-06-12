import { Toaster as Sonner, type ToasterProps } from "sonner"

// The panel is dark-fixed (main.tsx pins `.dark` on documentElement), so the
// toaster hardcodes the dark theme and maps sonner's surface vars onto the
// app's oklch popover/border tokens (index.css). Zero CDN — sonner is bundled.
function Toaster(props: ToasterProps) {
  return (
    <Sonner
      theme="dark"
      className="toaster group"
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
        } as React.CSSProperties
      }
      {...props}
    />
  )
}

export { Toaster }
