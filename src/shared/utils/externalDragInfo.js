import { createDragPreviewIcon, createImagesDragPreviewIcon } from './dragPreviewIcon';

export function getExternalDragInfo(item, renderType, t) {
  const isImage = renderType === 'image';
  const isFile = renderType === 'file';
  if ((!isImage && !isFile) || !item.content?.startsWith('files:')) {
    return { paths: [], iconPath: null };
  }

  try {
    const files = JSON.parse(item.content.substring(6)).files || [];
    if (isImage) {
      const first = files[0];
      const path = first?.exists === false ? null : first?.actual_path || first?.path;
      return path ? { paths: [path], iconPath: ({ paths }) => createImagesDragPreviewIcon(paths) } : { paths: [], iconPath: null };
    }

    const draggableFiles = files.filter(file => file.exists !== false && file.path);
    const paths = draggableFiles.map(file => file.path);
    if (!paths.length) return { paths: [], iconPath: null };
    const previewIcon = draggableFiles.find(file => file.icon_data)?.icon_data || '';
    return {
      paths,
      iconPath: ({ paths: previewPaths, mode }) => createDragPreviewIcon(previewIcon, previewPaths.length, mode, {
        copy: t('common.copy', '复制'),
        move: t('transferShelf.move', '移动'),
      }) || previewPaths[0],
    };
  } catch {
    return { paths: [], iconPath: null };
  }
}
