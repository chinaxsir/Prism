/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        surface: {
          DEFAULT: "#12141A",
          card: "#1E222D",
          hover: "#252936",
        },
        accent: {
          DEFAULT: "#6366f1",
          hover: "#818cf8",
        },
        latency: {
          good: "#22c55e",
          medium: "#eab308",
          bad: "#ef4444",
        },
        ring: "#6366f1",
      },
    },
  },
  plugins: [],
};
