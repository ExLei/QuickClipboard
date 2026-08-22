import { useCallback, useEffect, useRef, useState } from 'react';
import { startDrag } from '@crabnebula/tauri-plugin-drag';

const EXTERNAL_DRAG_EDGE_THRESHOLD_PX = 24;

export function useExternalDragSwitch({ onDragStart, onDragEnd, onDragCancel, closePreview }) {
  const [dndContextKey, setDndContextKey] = useState(0);
  const [showSafeZones, setShowSafeZones] = useState(false);
  const switchingRef = useRef(false);
  const previewRef = useRef(null);
  const activeDragRef = useRef(null);
  const previousClipPathRef = useRef('');

  const clearDragVisuals = useCallback(() => {
    document.body.classList.remove('dragging-cursor');
    document.body.style.clipPath = previousClipPathRef.current;
  }, []);

  const createPreview = useCallback(async (dragInfo) => {
    try {
      if (typeof dragInfo?.iconPath === 'function') {
        return await dragInfo.iconPath({ paths: dragInfo.paths, mode: 'copy' });
      }
      return dragInfo?.iconPath || dragInfo?.paths?.[0];
    } catch (error) {
      console.error('生成系统拖拽预览失败:', error);
      return dragInfo?.paths?.[0];
    }
  }, []);

  const prepareExternalDrag = useCallback((sortId, dragInfo) => {
    if (!sortId || !dragInfo?.paths?.length || previewRef.current?.sortId === sortId) return;
    previewRef.current = { sortId, promise: createPreview(dragInfo) };
  }, [createPreview]);

  const switchToExternalDrag = useCallback((sortId, dragInfo) => {
    if (switchingRef.current || !dragInfo?.paths?.length) return;
    switchingRef.current = true;
    activeDragRef.current = null;
    setShowSafeZones(false);
    clearDragVisuals();
    onDragCancel();
    setDndContextKey(key => key + 1);
    closePreview?.();

    (async () => {
      try {
        const preview = previewRef.current;
        const icon = preview?.sortId === sortId ? await preview.promise : await createPreview(dragInfo);
        await startDrag({ item: dragInfo.paths, icon: icon || dragInfo.paths[0], mode: 'copy' });
      } catch (error) {
        console.error('启动系统拖拽失败:', error);
      } finally {
        switchingRef.current = false;
        previewRef.current = null;
      }
    })();
  }, [clearDragVisuals, closePreview, createPreview, onDragCancel]);

  useEffect(() => {
    const handleMouseMove = (event) => {
      const activeDrag = activeDragRef.current;
      if (!activeDrag || switchingRef.current) return;
      if (event.clientX <= EXTERNAL_DRAG_EDGE_THRESHOLD_PX || event.clientX >= window.innerWidth - EXTERNAL_DRAG_EDGE_THRESHOLD_PX) {
        switchToExternalDrag(activeDrag.sortId, activeDrag.dragInfo);
      }
    };
    document.addEventListener('mousemove', handleMouseMove, true);
    return () => document.removeEventListener('mousemove', handleMouseMove, true);
  }, [switchToExternalDrag]);

  const handleDndDragStart = useCallback((event) => {
    const dragInfo = event.active.data.current?.externalDrag;
    prepareExternalDrag(event.active.id, dragInfo);
    setShowSafeZones(Boolean(dragInfo?.paths?.length));
    activeDragRef.current = dragInfo?.paths?.length ? { sortId: event.active.id, dragInfo } : null;
    previousClipPathRef.current = document.body.style.clipPath;
    document.body.classList.add('dragging-cursor');
    document.body.style.clipPath = 'inset(5px round 8px)';
    onDragStart(event);
  }, [onDragStart, prepareExternalDrag]);

  const handleDndDragEnd = useCallback((event) => {
    activeDragRef.current = null;
    setShowSafeZones(false);
    clearDragVisuals();
    onDragEnd(event);
  }, [clearDragVisuals, onDragEnd]);

  const handleDndDragCancel = useCallback((event) => {
    activeDragRef.current = null;
    setShowSafeZones(false);
    clearDragVisuals();
    onDragCancel(event);
  }, [clearDragVisuals, onDragCancel]);

  return {
    dndContextKey,
    showSafeZones,
    prepareExternalDrag,
    handleDndDragStart,
    handleDndDragEnd,
    handleDndDragCancel,
  };
}
