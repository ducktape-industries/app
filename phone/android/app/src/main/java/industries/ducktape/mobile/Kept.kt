package industries.ducktape.mobile

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import java.io.File
import java.io.FileNotFoundException

/**
 * The files the host hands the system's viewer (`MainActivity.keepFile`), read-only, to whom a viewing intent grants
 * them: each one under `<cache>/ducktape/kept/<folder>/<name>`, where the host writes a file to keep and removes it
 * once the viewer closes (`crates/host/src/runtime/picker.rs` `keeping_path`). Nothing outside that folder is served.
 * This app uses no AndroidX, so this is its `FileProvider`.
 */
class Kept : ContentProvider() {
    override fun onCreate() = true

    /** The file [uri] names, if it is one the host keeps. */
    private fun file(uri: Uri): File? {
        val root = File(context?.cacheDir ?: return null, KEPT).canonicalFile
        val segments = uri.pathSegments
        if (segments.size != 2) return null
        val file = File(File(root, segments[0]), segments[1]).canonicalFile
        return file.takeIf { it.parentFile?.parentFile == root && it.isFile }
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("the kept files are read-only")
        val file = file(uri) ?: throw FileNotFoundException(uri.toString())
        return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    }

    override fun getType(uri: Uri): String? =
        file(uri)?.let { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it.extension.lowercase()) }

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? {
        val file = file(uri) ?: return null
        val cursor = MatrixCursor(arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE))
        cursor.addRow(arrayOf<Any>(file.name, file.length()))
        return cursor
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0

    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0

    companion object {
        /** The provider's authority (AndroidManifest.xml) and the kept files' folder under the cache. */
        const val AUTHORITY = "industries.ducktape.mobile.kept"
        const val KEPT = "ducktape/kept"

        /** The content URI [file] (a kept file) is viewed by. */
        fun uri(file: File): Uri = Uri.Builder()
            .scheme("content")
            .authority(AUTHORITY)
            .appendPath(file.parentFile?.name ?: "")
            .appendPath(file.name)
            .build()
    }
}
