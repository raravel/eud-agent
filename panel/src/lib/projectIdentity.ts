import { useEffect, useRef } from "react";

/** Run a lifecycle effect when the connected editor-project identity changes. */
export function useProjectIdentityEffect(
  projectIdentity: string | null,
  onProjectIdentity: (projectIdentity: string | null) => void,
): void {
  const effectRef = useRef(onProjectIdentity);
  effectRef.current = onProjectIdentity;

  useEffect(() => {
    effectRef.current(projectIdentity);
  }, [projectIdentity]);
}
